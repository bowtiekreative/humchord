mod audio;
mod daw;
mod engine;
mod gui;
mod gui_daw;
mod sample;
mod spec;
mod synth;
mod theory;
mod yin;

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use audio::{ci1_present, Capture, Playback, CI1_SINK, CI1_SOURCE};
use synth::ChordSynth;
use theory::{
    format_chord, freq_to_midi, kind_by_slug, midi_to_name, print_all_chords, ChordKind, CHORD_KINDS,
};
use yin::{rms, yin};

#[derive(Clone, Copy, ValueEnum)]
enum ChordOpt {
    Major,
    Minor,
    Maj7,
    Min7,
    Dominant,
    Dim,
    Aug,
    Sus4,
}

impl ChordOpt {
    fn kind(self) -> &'static ChordKind {
        let slug = match self {
            ChordOpt::Major => "major",
            ChordOpt::Minor => "minor",
            ChordOpt::Maj7 => "maj7",
            ChordOpt::Min7 => "min7",
            ChordOpt::Dominant => "7",
            ChordOpt::Dim => "dim",
            ChordOpt::Aug => "aug",
            ChordOpt::Sus4 => "sus4",
        };
        kind_by_slug(slug).expect("chord slug")
    }
}

#[derive(Parser)]
#[command(
    name = "humchord",
    about = "Listen to a hummed pitch, detect the fundamental, and generate/play harmonizing chords.",
    after_help = "Default audio is the Steinberg CI1 when it is plugged in (PipeWire).\nHum a steady note. MIX on the interface toward the middle to hear yourself + the chord."
)]
struct Args {
    /// Analyze a WAV file instead of the live microphone
    file: Option<String>,

    /// Chord quality to play (all qualities are always printed)
    #[arg(short, long, value_enum, default_value = "major")]
    chord: ChordOpt,

    /// Detect only — do not synthesize chords
    #[arg(long)]
    no_play: bool,

    /// Sample rate
    #[arg(long, default_value_t = 48000)]
    rate: u32,

    /// PipeWire capture target (default: CI1 if present)
    #[arg(long)]
    input: Option<String>,

    /// PipeWire playback target (default: CI1 if present)
    #[arg(long)]
    output: Option<String>,

    /// YIN voiced threshold (lower = more sensitive)
    #[arg(long, default_value_t = 0.15)]
    threshold: f32,

    /// RMS gate; below this is treated as silence
    #[arg(long, default_value_t = 0.008)]
    gate: f32,

    /// Terminal UI instead of the window
    #[arg(long)]
    cli: bool,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("humchord: {e:?}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    if let Some(path) = args.file.as_deref() {
        return analyze_file(path, args.threshold, args.gate);
    }
    if args.cli {
        return live(args);
    }
    let kind_i = crate::theory::CHORD_KINDS
        .iter()
        .position(|k| k.slug == args.chord.kind().slug)
        .unwrap_or(0);
    gui::run(!args.no_play, kind_i, args.rate, args.threshold, args.gate)
}

fn analyze_file(path: &str, threshold: f32, gate: f32) -> Result<()> {
    let (mono, sr) = audio::load_wav_mono_f32(path)?;
    let win = ((sr as f32 * 0.046).round() as usize).clamp(1024, 8192);
    let hop = win / 2;
    let mut pitches = Vec::new();
    let mut i = 0;
    while i + win <= mono.len() {
        let frame = &mono[i..i + win];
        if rms(frame) >= gate {
            if let Some((f0, conf)) = yin(frame, sr as f32, 65.4, 1046.5, threshold) {
                if conf > 0.4 {
                    pitches.push(f0);
                }
            }
        }
        i += hop;
    }
    if pitches.is_empty() {
        bail!("No clear pitch detected. Hum louder, longer, and closer to a single note.");
    }
    pitches.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let avg = pitches[pitches.len() / 2];
    let midi = freq_to_midi(avg).round() as i32;
    println!("Detected hum frequency: {avg:.2} Hz");
    println!("Closest musical note: {} (MIDI {midi})", midi_to_name(midi));
    println!("----------------------------------------");
    println!("Chord options for this root:");
    print_all_chords(midi);
    Ok(())
}

fn live(args: Args) -> Result<()> {
    let running = Arc::new(AtomicBool::new(true));
    {
        let flag = running.clone();
        ctrlc::set_handler(move || {
            flag.store(false, Ordering::SeqCst);
        })
        .ok();
    }

    let use_ci1 = ci1_present();
    let input = args
        .input
        .clone()
        .or_else(|| use_ci1.then(|| CI1_SOURCE.to_string()));
    let output = args
        .output
        .clone()
        .or_else(|| use_ci1.then(|| CI1_SINK.to_string()));

    let rate = args.rate;
    let win = 2048usize;
    let hop = 1024usize;
    let latency = "40ms";
    let kind = args.chord.kind();

    println!("humchord  •  live YIN pitch → chords");
    println!(
        "input:  {}",
        input.as_deref().unwrap_or("(PipeWire default)")
    );
    println!(
        "output: {}",
        if args.no_play {
            "(off)"
        } else {
            output.as_deref().unwrap_or("(PipeWire default)")
        }
    );
    println!("playing: {}  •  hum a steady note  •  Ctrl-C to quit", kind.name);
    println!();

    let mut cap = Capture::start(rate, 1, input.as_deref(), latency)?;
    let mut play = if args.no_play {
        None
    } else {
        Some(Playback::start(rate, 2, output.as_deref(), latency)?)
    };
    let mut synth = ChordSynth::new(rate as f32, 0.18);

    let mut ring = vec![0.0f32; win];
    let mut chunk = vec![0i16; hop];
    let mut play_buf = vec![0i16; hop * 2];
    let mut last_midi: Option<i32> = None;
    let mut hold: u8 = 0;

    while running.load(Ordering::SeqCst) {
        if cap.read_exact_s16(&mut chunk).is_err() {
            break;
        }
        ring.rotate_left(hop);
        let start = win - hop;
        for (i, s) in chunk.iter().enumerate() {
            ring[start + i] = *s as f32 / 32768.0;
        }

        let energy = rms(&ring);
        let mut voiced = None;
        if energy >= args.gate {
            if let Some((f0, conf)) = yin(&ring, rate as f32, 65.4, 1046.5, args.threshold) {
                if conf > 0.45 {
                    voiced = Some((f0, conf));
                }
            }
        }

        match voiced {
            Some((f0, conf)) => {
                let midi = freq_to_midi(f0).round() as i32;
                if Some(midi) != last_midi {
                    hold += 1;
                    if hold >= 2 {
                        last_midi = Some(midi);
                        hold = 0;
                        synth.set_chord(midi, kind);
                        redraw(f0, conf, energy, midi, kind)?;
                    }
                } else {
                    hold = 0;
                }
            }
            None => {
                hold = 0;
                if last_midi.take().is_some() {
                    synth.silence();
                    eprint!("\r{:<80}\r", "… silence");
                    let _ = io::stderr().flush();
                }
            }
        }

        if let Some(p) = play.as_mut() {
            synth.render(&mut play_buf);
            p.write_s16(&play_buf)?;
        }
    }
    eprintln!();
    Ok(())
}

fn redraw(f0: f32, conf: f32, energy: f32, midi: i32, playing: &ChordKind) -> Result<()> {
    let mut out = String::new();
    out.push('\n');
    out.push_str(&format!(
        "f0 {f0:7.2} Hz   {}   conf {:.0}%   rms {:.3}\n",
        midi_to_name(midi),
        conf * 100.0,
        energy
    ));
    for kind in CHORD_KINDS {
        let mark = if kind.slug == playing.slug { "▶" } else { " " };
        out.push_str(&format!("{mark} {}\n", format_chord(midi, kind)));
    }
    print!("{out}");
    io::stdout().flush().context("stdout")?;
    Ok(())
}


