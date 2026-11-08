//! Record → analyze → chord preview. No live tracking.

use anyhow::Result;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::audio::{ci1_present, write_wav_mono, Capture, Playback, CI1_SINK, CI1_SOURCE};
use crate::daw::Song;
use crate::sample::Sample;
use crate::spec::{mag_at, Stft};
use crate::synth::ChordSynth;
use crate::theory::{freq_to_midi, midi_to_freq, midi_to_name, CHORD_KINDS};
use crate::yin::{rms, yin};

pub const SPEC_BINS: usize = 96;
pub const F_MAX: f32 = 2000.0;

#[derive(Clone)]
pub struct Partial {
    pub n: u8,
    pub hz: f32,
    pub midi: i32,
    pub name: String,
    pub mag: f32,
}

#[derive(Clone)]
pub struct DetectedNote {
    pub midi: i32,
    pub name: String,
    pub hz: f32,
    pub frames: u32,
    pub strength: f32,
}

#[derive(Clone)]
pub struct Analysis {
    pub spec: Vec<Vec<f32>>,
    pub f0: Vec<Option<f32>>,
    pub root: i32,
    pub duration: f32,
    pub wave: Vec<f32>,
    pub partials: Vec<Partial>,
    pub notes: Vec<DetectedNote>,
    pub peaks: Vec<(usize, f32)>,
}

pub struct Shared {
    pub stop: AtomicBool,
    pub recording: AtomicBool,
    pub rec_frames: AtomicU32,
    pub preview_left: AtomicU32,
    pub preview_kind: AtomicUsize,
    pub preview_root: AtomicI32,
    pub save_req: AtomicBool,
    pub ci1: AtomicBool,
    pub err: Mutex<Option<String>>,
    pub rec_buf: Mutex<Vec<f32>>,
    pub rec_full: Mutex<Vec<f32>>,
    pub rec_path: Mutex<Option<String>>,
    pub analysis: Mutex<Option<Analysis>>,
    pub queue: Mutex<VecDeque<(Vec<i32>, u32)>>,
    pub prog_step: AtomicI32,
    pub preview_midis: Mutex<Vec<i32>>,
    pub song: Mutex<Song>,
    pub sampler: Mutex<Option<Sample>>,
    pub daw_play: AtomicBool,
    pub playhead: AtomicU64,
}

impl Shared {
    pub fn new(_play: bool, _kind: usize) -> Arc<Self> {
        Arc::new(Self {
            stop: AtomicBool::new(false),
            recording: AtomicBool::new(false),
            rec_frames: AtomicU32::new(0),
            preview_left: AtomicU32::new(0),
            preview_kind: AtomicUsize::new(0),
            preview_root: AtomicI32::new(69),
            save_req: AtomicBool::new(false),
            ci1: AtomicBool::new(ci1_present()),
            err: Mutex::new(None),
            rec_buf: Mutex::new(Vec::new()),
            rec_full: Mutex::new(Vec::new()),
            rec_path: Mutex::new(None),
            analysis: Mutex::new(None),
            queue: Mutex::new(VecDeque::new()),
            prog_step: AtomicI32::new(-1),
            preview_midis: Mutex::new(Vec::new()),
            song: Mutex::new(Song::new()),
            sampler: Mutex::new(None),
            daw_play: AtomicBool::new(false),
            playhead: AtomicU64::new(0),
        })
    }
}

pub fn spawn(shared: Arc<Shared>, rate: u32, _threshold: f32, _gate: f32) -> JoinHandle<()> {
    thread::spawn(move || {
        if let Err(e) = run(shared.clone(), rate) {
            if let Ok(mut g) = shared.err.lock() {
                *g = Some(e.to_string());
            }
        }
    })
}

fn run(shared: Arc<Shared>, rate: u32) -> Result<()> {
    let use_ci1 = ci1_present();
    shared.ci1.store(use_ci1, Ordering::Relaxed);
    let input = use_ci1.then_some(CI1_SOURCE);
    let output = use_ci1.then_some(CI1_SINK);
    let hop = 1024usize;
    let latency = "40ms";
    let hop_dur = Duration::from_secs_f64(hop as f64 / rate as f64);

    let mut cap: Option<Capture> = None;
    let mut play_dev: Option<Playback> = None;
    let mut synth = ChordSynth::new(rate as f32, 0.22);
    let mut chunk = vec![0i16; hop];
    let mut play_buf = vec![0i16; hop * 2];
    let mut was_rec = false;

    while !shared.stop.load(Ordering::SeqCst) {
        let rec = shared.recording.load(Ordering::Relaxed);

        if rec {
            if cap.is_none() {
                cap = Some(Capture::start(rate, 1, input, latency)?);
                if let Ok(mut b) = shared.rec_buf.lock() {
                    b.clear();
                }
                shared.rec_frames.store(0, Ordering::Relaxed);
                if let Ok(mut a) = shared.analysis.lock() {
                    *a = None;
                }
            }
            if cap.as_mut().unwrap().read_exact_s16(&mut chunk).is_err() {
                if let Ok(mut g) = shared.err.lock() {
                    *g = Some("capture ended".into());
                }
                shared.recording.store(false, Ordering::Relaxed);
                cap = None;
                continue;
            }
            let mut f32s = Vec::with_capacity(hop);
            for s in &chunk {
                f32s.push(*s as f32 / 32768.0);
            }
            if let Ok(mut b) = shared.rec_buf.lock() {
                b.extend_from_slice(&f32s);
            }
            shared.rec_frames.fetch_add(hop as u32, Ordering::Relaxed);
        } else {
            cap = None;
            if was_rec || shared.save_req.swap(false, Ordering::SeqCst) {
                finish_take(&shared, rate);
            }
        }
        was_rec = rec;

        let preview = shared.preview_left.load(Ordering::Relaxed);
        if preview > 0 {
            let midis = shared
                .preview_midis
                .lock()
                .ok()
                .map(|g| g.clone())
                .unwrap_or_default();
            if !midis.is_empty() {
                synth.set_midis(&midis);
            } else {
                let kind_i = shared.preview_kind.load(Ordering::Relaxed).min(CHORD_KINDS.len() - 1);
                let root = shared.preview_root.load(Ordering::Relaxed);
                synth.set_chord(root, &CHORD_KINDS[kind_i]);
            }
            if play_dev.is_none() {
                play_dev = Playback::start(rate, 2, output, latency).ok();
            }
            if let Some(p) = play_dev.as_mut() {
                synth.render(&mut play_buf);
                let _ = p.write_s16(&play_buf);
            }
            shared
                .preview_left
                .store(preview.saturating_sub(hop as u32), Ordering::Relaxed);
        } else {
            let mut started = false;
            if let Ok(mut q) = shared.queue.lock() {
                if let Some((midis, dur)) = q.pop_front() {
                    if let Ok(mut m) = shared.preview_midis.lock() {
                        *m = midis;
                    }
                    shared.preview_left.store(dur, Ordering::Relaxed);
                    shared.prog_step.fetch_add(1, Ordering::Relaxed);
                    started = true;
                } else if shared.prog_step.load(Ordering::Relaxed) >= 0 {
                    shared.prog_step.store(-1, Ordering::Relaxed);
                }
            }
            if started {
                continue;
            }
            if shared.daw_play.load(Ordering::Relaxed) {
                let start = shared.playhead.load(Ordering::Relaxed);
                let mix = match (shared.song.lock(), shared.sampler.lock()) {
                    (Ok(song), Ok(sm)) => song.mix_block(start, hop, sm.as_ref()),
                    _ => vec![0.0f32; hop],
                };
                if play_dev.is_none() {
                    play_dev = Playback::start(rate, 2, output, latency).ok();
                }
                let mut stereo = Vec::with_capacity(hop * 2);
                for s in &mix {
                    let v = (*s * 32767.0).clamp(-32767.0, 32767.0) as i16;
                    stereo.push(v);
                    stereo.push(v);
                }
                if let Some(p) = play_dev.as_mut() {
                    let _ = p.write_s16(&stereo);
                }
                let next = start + hop as u64;
                shared.playhead.store(next, Ordering::Relaxed);
                let max = shared
                    .song
                    .lock()
                    .ok()
                    .map(|s| (s.length_beats() * 60.0 / s.bpm.max(1.0) * rate as f32) as u64)
                    .unwrap_or(rate as u64 * 8);
                if next > max {
                    shared.daw_play.store(false, Ordering::Relaxed);
                    shared.playhead.store(0, Ordering::Relaxed);
                }
            } else {
                play_dev = None;
                synth.silence();
                if !rec {
                    thread::sleep(Duration::from_millis(20));
                }
            }
        }
        let _ = hop_dur;
    }
    Ok(())
}

fn finish_take(shared: &Shared, rate: u32) {
    let samples = shared
        .rec_buf
        .lock()
        .ok()
        .map(|g| g.clone())
        .unwrap_or_default();
    if samples.len() < rate as usize / 5 {
        if let Ok(mut g) = shared.err.lock() {
            *g = Some("recording too short — hold for at least a second".into());
        }
        return;
    }
    if let Ok(mut g) = shared.rec_full.lock() {
        *g = samples.clone();
    }
    commit_samples(shared, rate, &samples);
}

impl Shared {
    /// Keep only [lo, hi] of the original take (0..1) and re-analyze.
    pub fn apply_clip(&self, lo: f32, hi: f32, rate: u32) {
        let full = self.rec_full.lock().ok().map(|g| g.clone()).unwrap_or_default();
        if full.is_empty() {
            return;
        }
        let n = full.len();
        let mut a = ((lo.clamp(0.0, 1.0)) * n as f32) as usize;
        let mut b = ((hi.clamp(0.0, 1.0)) * n as f32) as usize;
        if b < a {
            std::mem::swap(&mut a, &mut b);
        }
        if b.saturating_sub(a) < rate as usize / 5 {
            if let Ok(mut g) = self.err.lock() {
                *g = Some("clip too short".into());
            }
            return;
        }
        let slice = full[a..b].to_vec();
        if let Ok(mut g) = self.rec_buf.lock() {
            *g = slice.clone();
        }
        commit_samples(self, rate, &slice);
    }

    pub fn play_progression(&self, steps: &[Vec<i32>], beat: u32) {
        if let Ok(mut q) = self.queue.lock() {
            q.clear();
            for notes in steps {
                q.push_back((notes.clone(), beat));
            }
        }
        self.prog_step.store(-1, Ordering::Relaxed);
        self.preview_left.store(0, Ordering::Relaxed);
    }

    pub fn play_notes(&self, notes: Vec<i32>, samples: u32) {
        if let Ok(mut m) = self.preview_midis.lock() {
            *m = notes;
        }
        self.preview_left.store(samples, Ordering::Relaxed);
    }

    pub fn stop_progression(&self) {
        if let Ok(mut q) = self.queue.lock() {
            q.clear();
        }
        self.preview_left.store(0, Ordering::Relaxed);
        self.prog_step.store(-1, Ordering::Relaxed);
    }
}

fn commit_samples(shared: &Shared, rate: u32, samples: &[f32]) {
    let dir = dirs_music();
    let _ = std::fs::create_dir_all(&dir);
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("humchord-{t}.wav"));
    let p = path.to_string_lossy().to_string();
    if let Err(e) = write_wav_mono(&p, rate, samples) {
        if let Ok(mut g) = shared.err.lock() {
            *g = Some(e.to_string());
        }
        return;
    }
    if let Ok(mut g) = shared.rec_path.lock() {
        *g = Some(p);
    }
    match analyze_buffer(samples, rate) {
        Some(a) => {
            shared.preview_root.store(a.root, Ordering::Relaxed);
            if let Ok(mut g) = shared.analysis.lock() {
                *g = Some(a);
            }
            if let Ok(mut g) = shared.err.lock() {
                *g = None;
            }
        }
        None => {
            if let Ok(mut g) = shared.err.lock() {
                *g = Some("no pitch in this clip — try a longer or louder hum".into());
            }
        }
    }
}

pub fn analyze_buffer(samples: &[f32], sr: u32) -> Option<Analysis> {
    let win = 2048usize;
    let hop = 512usize;
    if samples.len() < win {
        return None;
    }
    let mut stft = Stft::new(win);
    let mut spec = Vec::new();
    let mut f0s = Vec::new();
    let mut voiced_f = Vec::new();
    let mut i = 0;
    while i + win <= samples.len() {
        let frame = &samples[i..i + win];
        spec.push(stft.column(frame, sr as f32, SPEC_BINS, F_MAX));
        if rms(frame) >= 0.008 {
            if let Some((f, c)) = yin(frame, sr as f32, 65.4, 1046.5, 0.15) {
                if c > 0.45 {
                    f0s.push(Some(f));
                    voiced_f.push(f);
                } else {
                    f0s.push(None);
                }
            } else {
                f0s.push(None);
            }
        } else {
            f0s.push(None);
        }
        i += hop;
    }
    let (notes, peaks) = pick_notes(&spec);
    if voiced_f.is_empty() && notes.is_empty() {
        return None;
    }
    let root = if !voiced_f.is_empty() {
        voiced_f.sort_by(|a, b| a.partial_cmp(b).unwrap());
        freq_to_midi(voiced_f[voiced_f.len() / 2]).round() as i32
    } else {
        notes[0].midi
    };
    let med = midi_to_freq(root as f32);
    let nwave = 800.min(samples.len());
    let step = (samples.len() / nwave).max(1);
    let wave: Vec<f32> = (0..nwave).map(|k| samples[k * step]).collect();
    let mut partials = Vec::new();
    for n in 1u8..=8 {
        let hz = med * n as f32;
        if hz > F_MAX {
            break;
        }
        let mag = mag_at(samples, sr as f32, hz);
        let midi = freq_to_midi(hz).round() as i32;
        partials.push(Partial {
            n,
            hz,
            midi,
            name: midi_to_name(midi),
            mag,
        });
    }
    Some(Analysis {
        spec,
        f0: f0s,
        root,
        duration: samples.len() as f32 / sr as f32,
        wave,
        partials,
        notes,
        peaks,
    })
}

fn pick_notes(spec: &[Vec<f32>]) -> (Vec<DetectedNote>, Vec<(usize, f32)>) {
    use std::collections::HashMap;
    let mut acc: HashMap<i32, (f32, u32, f32)> = HashMap::new();
    let mut peaks = Vec::new();
    for (t, col) in spec.iter().enumerate() {
        let mx = col.iter().copied().fold(0.0f32, f32::max);
        if mx < 0.22 {
            continue;
        }
        let thr = (mx * 0.42).max(0.26);
        let n = col.len();
        for b in 2..n.saturating_sub(2) {
            if col[b] >= thr
                && col[b] >= col[b - 1]
                && col[b] >= col[b + 1]
                && col[b] >= col[b - 2]
                && col[b] >= col[b + 2]
            {
                let hz = (b as f32 + 0.5) / n as f32 * F_MAX;
                if !(80.0..=F_MAX).contains(&hz) {
                    continue;
                }
                let midi = freq_to_midi(hz).round() as i32;
                if !(36..=96).contains(&midi) {
                    continue;
                }
                peaks.push((t, hz));
                let e = acc.entry(midi).or_insert((0.0, 0, 0.0));
                e.0 += col[b];
                e.1 += 1;
                e.2 += hz;
            }
        }
    }
    let mut notes: Vec<DetectedNote> = acc
        .into_iter()
        .filter(|(_, v)| v.1 >= 2)
        .map(|(midi, (s, n, hsum))| DetectedNote {
            midi,
            name: midi_to_name(midi),
            hz: hsum / n as f32,
            frames: n,
            strength: s,
        })
        .collect();
    notes.sort_by(|a, b| b.strength.partial_cmp(&a.strength).unwrap());
    (notes, peaks)
}

fn dirs_music() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let p = PathBuf::from(&home).join("Music");
    if p.is_dir() {
        p
    } else {
        PathBuf::from(home)
    }
}
