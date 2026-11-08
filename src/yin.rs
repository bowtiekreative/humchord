//! YIN fundamental-frequency estimator (de Cheveigné & Kawahara, 2002).

/// Returns `(frequency_hz, confidence 0..1)` or `None` if unvoiced.
pub fn yin(samples: &[f32], sr: f32, fmin: f32, fmax: f32, threshold: f32) -> Option<(f32, f32)> {
    let n = samples.len();
    if n < 64 {
        return None;
    }
    let tau_min = (sr / fmax).floor().max(2.0) as usize;
    let tau_max = ((sr / fmin).floor() as usize).min(n / 2);
    if tau_max <= tau_min + 2 {
        return None;
    }

    let mut d = vec![0.0f32; tau_max + 1];
    let w = n - tau_max;
    for tau in tau_min..=tau_max {
        let mut sum = 0.0f32;
        for j in 0..w {
            let diff = samples[j] - samples[j + tau];
            sum += diff * diff;
        }
        d[tau] = sum;
    }

    let mut cmnd = vec![1.0f32; tau_max + 1];
    let mut running = 0.0f32;
    for tau in 1..=tau_max {
        running += d[tau];
        cmnd[tau] = if running > 1e-12 {
            d[tau] * tau as f32 / running
        } else {
            1.0
        };
    }

    let mut tau = tau_min;
    let mut found = false;
    while tau < tau_max {
        if cmnd[tau] < threshold {
            while tau + 1 < tau_max && cmnd[tau + 1] < cmnd[tau] {
                tau += 1;
            }
            found = true;
            break;
        }
        tau += 1;
    }
    if !found || cmnd[tau] >= threshold {
        return None;
    }

    let s0 = cmnd[tau.saturating_sub(1)];
    let s1 = cmnd[tau];
    let s2 = if tau + 1 <= tau_max { cmnd[tau + 1] } else { s1 };
    let denom = 2.0 * s1 - s2 - s0;
    let better = if denom.abs() > 1e-12 {
        tau as f32 + (s2 - s0) / (2.0 * denom)
    } else {
        tau as f32
    };
    if better < 1.0 {
        return None;
    }
    let freq = sr / better;
    if !(fmin..=fmax).contains(&freq) {
        return None;
    }
    Some((freq, (1.0 - s1).clamp(0.0, 1.0)))
}

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let s: f32 = samples.iter().map(|x| x * x).sum();
    (s / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::yin;

    #[test]
    fn detects_a4() {
        let sr = 48000.0f32;
        let n = 4096;
        let samples: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * 440.0 * i as f32 / sr).sin())
            .collect();
        let (f, conf) = yin(&samples, sr, 65.4, 1046.5, 0.15).expect("voiced");
        assert!((f - 440.0).abs() < 2.0, "got {f}");
        assert!(conf > 0.8, "got {conf}");
    }
}
