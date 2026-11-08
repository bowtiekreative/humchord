//! Load any audio ffmpeg/hound understands; detect pitch for sampler mapping.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::theory::{freq_to_midi, midi_to_freq, midi_to_name};
use crate::yin::{rms, yin};

#[derive(Clone)]
pub struct Sample {
    pub name: String,
    pub samples: Vec<f32>,
    pub sr: u32,
    pub root_midi: i32,
    pub in_norm: f32,
    pub out_norm: f32,
}

impl Sample {
    pub fn load(path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "sample".into());
        let (samples, sr) = decode(path)?;
        if samples.len() < sr as usize / 10 {
            bail!("file too short");
        }
        let root_midi = detect_root(&samples, sr).unwrap_or(60);
        Ok(Self {
            name,
            samples,
            sr,
            root_midi,
            in_norm: 0.0,
            out_norm: 1.0,
        })
    }

    pub fn from_buffer(name: String, samples: Vec<f32>, sr: u32) -> Self {
        let root_midi = detect_root(&samples, sr).unwrap_or(60);
        Self {
            name,
            samples,
            sr,
            root_midi,
            in_norm: 0.0,
            out_norm: 1.0,
        }
    }

    /// Hidden range only — original buffer is never destroyed.
    pub fn audible(&self) -> &[f32] {
        let n = self.samples.len();
        let a = ((self.in_norm.clamp(0.0, 1.0)) * n as f32) as usize;
        let b = ((self.out_norm.clamp(0.0, 1.0)) * n as f32) as usize;
        let a = a.min(n);
        let b = b.min(n).max(a + 1);
        &self.samples[a..b]
    }

    /// Pitch-shift by resampling so `midi` matches detected root.
    pub fn at_midi(&self, midi: i32, t_sec: f32) -> f32 {
        let src = &self.samples;
        if src.is_empty() {
            return 0.0;
        }
        let ratio = midi_to_freq(midi as f32) / midi_to_freq(self.root_midi as f32);
        let idx = t_sec * ratio * self.sr as f32;
        if idx < 0.0 {
            return 0.0;
        }
        let i = idx.floor() as usize;
        if i + 1 >= src.len() {
            return 0.0;
        }
        let f = idx - i as f32;
        src[i] * (1.0 - f) + src[i + 1] * f
    }

    pub fn root_name(&self) -> String {
        midi_to_name(self.root_midi)
    }
}

fn decode(path: &Path) -> Result<(Vec<f32>, u32)> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "wav" {
        if let Ok(v) = crate::audio::load_wav_mono_f32(&path.to_string_lossy()) {
            return Ok(v);
        }
    }
    let ff = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-i",
            path.to_str().unwrap_or(""),
            "-f",
            "f32le",
            "-ac",
            "1",
            "-ar",
            "48000",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("ffmpeg (needed for mp3/others)")?;
    if !ff.status.success() {
        bail!(
            "decode failed: {}",
            String::from_utf8_lossy(&ff.stderr)
        );
    }
    let mut samples = Vec::with_capacity(ff.stdout.len() / 4);
    for c in ff.stdout.chunks_exact(4) {
        samples.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
    }
    Ok((samples, 48000))
}

fn detect_root(samples: &[f32], sr: u32) -> Option<i32> {
    let win = 2048usize;
    let hop = 512usize;
    let mut pitches = Vec::new();
    let mut i = 0;
    while i + win <= samples.len() {
        let frame = &samples[i..i + win];
        if rms(frame) >= 0.008 {
            if let Some((f, c)) = yin(frame, sr as f32, 40.0, 2000.0, 0.15) {
                if c > 0.4 {
                    pitches.push(f);
                }
            }
        }
        i += hop;
    }
    if pitches.is_empty() {
        return None;
    }
    pitches.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(freq_to_midi(pitches[pitches.len() / 2]).round() as i32)
}

pub fn pick_audio_file() -> Option<std::path::PathBuf> {
    let out = Command::new("zenity")
        .args([
            "--file-selection",
            "--title=Choose sample (wav, mp3, …)",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(s))
    }
}
