//! Tiny sequencer: MIDI clips + stretchable audio, mixed to one stereo stream.

use crate::sample::Sample;
use crate::theory::midi_to_freq;
use std::sync::Arc;

pub const SR: u32 = 48000;

#[derive(Clone)]
pub struct MidiClip {
    pub start_beats: f32,
    pub len_beats: f32,
    pub label: String,
    pub notes: Vec<i32>,
}

#[derive(Clone)]
pub struct AudioClip {
    pub start_beats: f32,
    pub len_beats: f32,
    pub sample: Arc<Sample>,
    pub in_norm: f32,
    pub out_norm: f32,
}

#[derive(Clone)]
pub struct Chan {
    pub name: String,
    pub gain: f32,
    pub mute: bool,
    pub solo: bool,
}

#[derive(Clone)]
pub struct Song {
    pub bpm: f32,
    pub midi: Vec<MidiClip>,
    pub audio: Vec<AudioClip>,
    pub midi_mix: Chan,
    pub audio_mix: Chan,
}

impl Song {
    pub fn new() -> Self {
        Self {
            bpm: 120.0,
            midi: Vec::new(),
            audio: Vec::new(),
            midi_mix: Chan {
                name: "MIDI".into(),
                gain: 0.85,
                mute: false,
                solo: false,
            },
            audio_mix: Chan {
                name: "Audio".into(),
                gain: 0.85,
                mute: false,
                solo: false,
            },
        }
    }

    pub fn beats_to_sec(&self, b: f32) -> f32 {
        b * 60.0 / self.bpm.max(1.0)
    }

    pub fn length_beats(&self) -> f32 {
        let m = self
            .midi
            .iter()
            .map(|c| c.start_beats + c.len_beats)
            .fold(8.0f32, f32::max);
        let a = self
            .audio
            .iter()
            .map(|c| c.start_beats + c.len_beats)
            .fold(8.0f32, f32::max);
        m.max(a).max(8.0)
    }

    pub fn mix_block(&self, start_sample: u64, n: usize, sampler: Option<&Sample>) -> Vec<f32> {
        let mut out = vec![0.0f32; n];
        let sr = SR as f32;
        let any_solo = self.midi_mix.solo || self.audio_mix.solo;
        let midi_on = !self.midi_mix.mute && (!any_solo || self.midi_mix.solo);
        let audio_on = !self.audio_mix.mute && (!any_solo || self.audio_mix.solo);

        for i in 0..n {
            let t = (start_sample + i as u64) as f32 / sr;
            let beat = t * self.bpm / 60.0;
            let mut s = 0.0f32;
            if midi_on {
                s += self.midi_at(beat, sampler) * self.midi_mix.gain;
            }
            if audio_on {
                s += self.audio_at(beat) * self.audio_mix.gain;
            }
            out[i] = s.clamp(-1.0, 1.0);
        }
        out
    }

    fn midi_at(&self, beat: f32, sampler: Option<&Sample>) -> f32 {
        let mut s = 0.0;
        for c in &self.midi {
            if beat < c.start_beats || beat >= c.start_beats + c.len_beats {
                continue;
            }
            let local = self.beats_to_sec(beat - c.start_beats);
            let n = c.notes.len().max(1) as f32;
            for &m in &c.notes {
                let v = if let Some(smp) = sampler {
                    smp.at_midi(m, local)
                } else {
                    (std::f32::consts::TAU * midi_to_freq(m as f32) * local).sin() * 0.15
                        * ((1.0 - local / self.beats_to_sec(c.len_beats)).clamp(0.0, 1.0))
                };
                s += v / n.sqrt();
            }
        }
        s
    }

    fn audio_at(&self, beat: f32) -> f32 {
        let mut s = 0.0;
        for c in &self.audio {
            if beat < c.start_beats || beat >= c.start_beats + c.len_beats {
                continue;
            }
            let u = (beat - c.start_beats) / c.len_beats.max(0.01);
            let src = {
                let n = c.sample.samples.len();
                let a = ((c.in_norm.clamp(0.0, 1.0)) * n as f32) as usize;
                let b = ((c.out_norm.clamp(0.0, 1.0)) * n as f32) as usize;
                let a = a.min(n);
                let b = b.min(n).max(a + 1);
                &c.sample.samples[a..b]
            };
            if src.is_empty() {
                continue;
            }
            let idx = u * (src.len() - 1) as f32;
            let i = idx.floor() as usize;
            let f = idx - i as f32;
            let a = src.get(i).copied().unwrap_or(0.0);
            let b = src.get(i + 1).copied().unwrap_or(a);
            s += a * (1.0 - f) + b * f;
        }
        s
    }
}
