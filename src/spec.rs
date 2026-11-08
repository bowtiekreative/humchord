//! Real STFT column for the spectrogram.

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

pub struct Stft {
    n: usize,
    fft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl Stft {
    pub fn new(n: usize) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n);
        let scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        Self { n, fft, scratch }
    }

    /// Returns `bins` log-magnitudes (0..1-ish) covering 0..f_max Hz.
    pub fn column(&mut self, samples: &[f32], sr: f32, bins: usize, f_max: f32) -> Vec<f32> {
        let n = self.n.min(samples.len());
        let mut buf = vec![Complex::default(); self.n];
        for i in 0..n {
            let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos();
            buf[i].re = samples[samples.len() - n + i] * w;
        }
        self.fft.process_with_scratch(&mut buf, &mut self.scratch);
        let nyquist = sr / 2.0;
        let max_bin = ((f_max / nyquist) * (self.n as f32 / 2.0)).clamp(1.0, self.n as f32 / 2.0);
        let mut out = vec![0.0f32; bins];
        for b in 0..bins {
            let lo = (b as f32 / bins as f32) * max_bin;
            let hi = ((b + 1) as f32 / bins as f32) * max_bin;
            let i0 = lo.floor() as usize;
            let i1 = hi.ceil() as usize;
            let mut acc = 0.0;
            let mut c = 0;
            for i in i0..i1.max(i0 + 1) {
                if i < buf.len() / 2 {
                    acc += buf[i].norm();
                    c += 1;
                }
            }
            let mag = if c > 0 { acc / c as f32 } else { 0.0 };
            out[b] = (1.0 + mag * 40.0).ln() / 4.0;
        }
        out
    }
}

/// Magnitude at frequency f (linear, not log).
pub fn mag_at(samples: &[f32], sr: f32, f: f32) -> f32 {
    if samples.is_empty() || f <= 0.0 {
        return 0.0;
    }
    let mut re = 0.0f32;
    let mut im = 0.0f32;
    let w = std::f32::consts::TAU * f / sr;
    for (i, &s) in samples.iter().enumerate() {
        let a = w * i as f32;
        re += s * a.cos();
        im += s * a.sin();
    }
    (re * re + im * im).sqrt() / samples.len() as f32
}
