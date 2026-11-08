//! PipeWire I/O via `pw-cat` / `pw-record` (no ALSA headers required).

use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub const CI1_SINK: &str = "alsa_output.usb-Yamaha_Corporation_Steinberg_CI1-01.analog-stereo";
pub const CI1_SOURCE: &str = "alsa_input.usb-Yamaha_Corporation_Steinberg_CI1-01.analog-stereo";

pub fn ci1_present() -> bool {
    std::fs::read_to_string("/proc/asound/cards")
        .map(|s| s.contains("[CI1"))
        .unwrap_or(false)
}

pub struct Capture {
    child: Child,
    stdout: ChildStdout,
}

impl Capture {
    pub fn start(rate: u32, channels: u16, target: Option<&str>, latency: &str) -> Result<Self> {
        let mut cmd = Command::new("pw-record");
        cmd.arg("--rate")
            .arg(rate.to_string())
            .arg("--channels")
            .arg(channels.to_string())
            .arg("--format")
            .arg("s16")
            .arg("--latency")
            .arg(latency)
            .arg("--media-role")
            .arg("Production")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(t) = target {
            cmd.arg("--target").arg(t);
        }
        cmd.arg("-");
        let mut child = cmd.spawn().context("spawn pw-record (is pipewire installed?)")?;
        let stdout = child.stdout.take().context("pw-record stdout")?;
        Ok(Self { child, stdout })
    }

    pub fn read_exact_s16(&mut self, dst: &mut [i16]) -> Result<()> {
        let mut bytes = vec![0u8; dst.len() * 2];
        self.stdout
            .read_exact(&mut bytes)
            .context("pw-record stream ended")?;
        for (i, chunk) in bytes.chunks_exact(2).enumerate() {
            dst[i] = i16::from_le_bytes([chunk[0], chunk[1]]);
        }
        Ok(())
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct Playback {
    child: Child,
    stdin: ChildStdin,
}

impl Playback {
    pub fn start(rate: u32, channels: u16, target: Option<&str>, latency: &str) -> Result<Self> {
        let mut cmd = Command::new("pw-cat");
        cmd.arg("--playback")
            .arg("--rate")
            .arg(rate.to_string())
            .arg("--channels")
            .arg(channels.to_string())
            .arg("--format")
            .arg("s16")
            .arg("--latency")
            .arg(latency)
            .arg("--media-role")
            .arg("Production")
            .stdin(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(t) = target {
            cmd.arg("--target").arg(t);
        }
        cmd.arg("-");
        let mut child = cmd.spawn().context("spawn pw-cat --playback")?;
        let stdin = child.stdin.take().context("pw-cat stdin")?;
        Ok(Self { child, stdin })
    }

    pub fn write_s16(&mut self, src: &[i16]) -> Result<()> {
        let mut bytes = Vec::with_capacity(src.len() * 2);
        for s in src {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        self.stdin.write_all(&bytes).context("pw-cat write")?;
        Ok(())
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn load_wav_mono_f32(path: &str) -> Result<(Vec<f32>, u32)> {
    let mut reader = hound::WavReader::open(path).with_context(|| format!("open {path}"))?;
    let spec = reader.spec();
    let sr = spec.sample_rate;
    let ch = spec.channels.max(1) as usize;
    let mut mono = Vec::new();
    match spec.sample_format {
        hound::SampleFormat::Int => match spec.bits_per_sample {
            16 => {
                for (i, s) in reader.samples::<i16>().enumerate() {
                    let v = s? as f32 / 32768.0;
                    if i % ch == 0 {
                        mono.push(v);
                    } else if let Some(last) = mono.last_mut() {
                        *last += v;
                    }
                }
                if ch > 1 {
                    for s in &mut mono {
                        *s /= ch as f32;
                    }
                }
            }
            24 | 32 => {
                for (i, s) in reader.samples::<i32>().enumerate() {
                    let v = s? as f32 / 2147483648.0;
                    if i % ch == 0 {
                        mono.push(v);
                    } else if let Some(last) = mono.last_mut() {
                        *last += v;
                    }
                }
                if ch > 1 {
                    for s in &mut mono {
                        *s /= ch as f32;
                    }
                }
            }
            bits => bail!("unsupported bit depth {bits}"),
        },
        hound::SampleFormat::Float => {
            for (i, s) in reader.samples::<f32>().enumerate() {
                let v = s?;
                if i % ch == 0 {
                    mono.push(v);
                } else if let Some(last) = mono.last_mut() {
                    *last += v;
                }
            }
            if ch > 1 {
                for s in &mut mono {
                    *s /= ch as f32;
                }
            }
        }
    }
    Ok((mono, sr))
}

pub fn write_wav_mono(path: &str, sr: u32, samples: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: sr,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).with_context(|| format!("create {path}"))?;
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        w.write_sample(v)?;
    }
    w.finalize()?;
    Ok(())
}
