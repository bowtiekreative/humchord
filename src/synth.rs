//! Simple additive sine synth for chord tones.

use crate::theory::{chord_midis, midi_to_freq, ChordKind};

pub struct ChordSynth {
    sr: f32,
    phases: Vec<f32>,
    freqs: Vec<f32>,
    target_freqs: Vec<f32>,
    gain: f32,
}

impl ChordSynth {
    pub fn new(sr: f32, gain: f32) -> Self {
        Self {
            sr,
            phases: Vec::new(),
            freqs: Vec::new(),
            target_freqs: Vec::new(),
            gain,
        }
    }

    pub fn set_chord(&mut self, root_midi: i32, kind: &ChordKind) {
        self.set_midis(&chord_midis(root_midi, kind));
    }

    pub fn set_midis(&mut self, midis: &[i32]) {
        let freqs: Vec<f32> = midis.iter().map(|m| midi_to_freq(*m as f32)).collect();
        if self.freqs.len() != freqs.len() {
            self.phases = vec![0.0; freqs.len()];
            self.freqs = freqs.clone();
        }
        self.target_freqs = freqs;
    }

    pub fn silence(&mut self) {
        self.target_freqs.clear();
        self.freqs.clear();
        self.phases.clear();
    }

    /// Fill interleaved stereo s16 samples.
    pub fn render(&mut self, dst: &mut [i16]) {
        let frames = dst.len() / 2;
        if self.freqs.is_empty() {
            dst.fill(0);
            return;
        }
        let n = self.freqs.len() as f32;
        let voice_gain = self.gain / n.sqrt();
        for i in 0..frames {
            for v in 0..self.freqs.len() {
                // glide toward target to avoid clicks
                if v < self.target_freqs.len() {
                    self.freqs[v] = self.freqs[v] * 0.995 + self.target_freqs[v] * 0.005;
                }
                let inc = std::f32::consts::TAU * self.freqs[v] / self.sr;
                self.phases[v] = (self.phases[v] + inc) % std::f32::consts::TAU;
            }
            let mut s = 0.0f32;
            for &ph in &self.phases {
                s += ph.sin();
            }
            s *= voice_gain;
            let v = (s * 32767.0).clamp(-32767.0, 32767.0) as i16;
            dst[i * 2] = v;
            dst[i * 2 + 1] = v;
        }
    }
}
