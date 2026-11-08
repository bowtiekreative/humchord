//! Pitch-class math and chord construction (12-TET, A4 = 440 Hz).

pub const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

#[derive(Clone, Copy, Debug)]
pub struct ChordKind {
    pub name: &'static str,
    pub slug: &'static str,
    pub intervals: &'static [i32],
}

pub const CHORD_KINDS: &[ChordKind] = &[
    ChordKind { name: "Major", slug: "major", intervals: &[0, 4, 7] },
    ChordKind { name: "Minor", slug: "minor", intervals: &[0, 3, 7] },
    ChordKind { name: "Major 7th", slug: "maj7", intervals: &[0, 4, 7, 11] },
    ChordKind { name: "Minor 7th", slug: "min7", intervals: &[0, 3, 7, 10] },
    ChordKind { name: "Dominant 7th", slug: "7", intervals: &[0, 4, 7, 10] },
    ChordKind { name: "Diminished", slug: "dim", intervals: &[0, 3, 6] },
    ChordKind { name: "Augmented", slug: "aug", intervals: &[0, 4, 8] },
    ChordKind { name: "Suspended 4th", slug: "sus4", intervals: &[0, 5, 7] },
];

pub fn freq_to_midi(freq: f32) -> f32 {
    69.0 + 12.0 * (freq / 440.0).log2()
}

pub fn midi_to_freq(midi: f32) -> f32 {
    440.0 * 2f32.powf((midi - 69.0) / 12.0)
}

pub fn midi_to_name(midi: i32) -> String {
    let pc = midi.rem_euclid(12) as usize;
    let oct = (midi / 12) - 1;
    format!("{}{}", NOTE_NAMES[pc], oct)
}

pub fn kind_by_slug(slug: &str) -> Option<&'static ChordKind> {
    CHORD_KINDS.iter().find(|k| k.slug.eq_ignore_ascii_case(slug) || k.name.eq_ignore_ascii_case(slug))
}

pub fn chord_midis(root: i32, kind: &ChordKind) -> Vec<i32> {
    kind.intervals.iter().map(|i| root + i).collect()
}

pub fn format_chord(root: i32, kind: &ChordKind) -> String {
    let notes: Vec<String> = chord_midis(root, kind).into_iter().map(midi_to_name).collect();
    format!("{} {}: {}", midi_to_name(root), kind.name, notes.join("  "))
}

pub fn print_all_chords(root: i32) {
    for kind in CHORD_KINDS {
        println!("  • {}", format_chord(root, kind));
    }
}

#[derive(Clone)]
pub struct Voicing {
    pub name: &'static str,
    pub intervals: Vec<i32>,
}

fn invert_close(iv: &[i32], n: usize) -> Vec<i32> {
    let mut v = iv.to_vec();
    v.sort_unstable();
    let n = n % v.len().max(1);
    for _ in 0..n {
        let x = v.remove(0) + 12;
        v.push(x);
        v.sort_unstable();
    }
    v
}

fn drop_from_top(iv: &[i32], from_top: usize) -> Option<Vec<i32>> {
    let mut v = iv.to_vec();
    v.sort_unstable();
    if v.len() < from_top {
        return None;
    }
    let i = v.len() - from_top;
    v[i] -= 12;
    v.sort_unstable();
    Some(v)
}

/// Close inversions plus open / drop voicings for a chord quality.
pub fn voicings_for(kind: &ChordKind) -> Vec<Voicing> {
    let iv = kind.intervals;
    let n = iv.len();
    let inv_names = ["close root", "1st inversion", "2nd inversion", "3rd inversion"];
    let mut out = Vec::new();
    for i in 0..n {
        out.push(Voicing {
            name: inv_names[i.min(3)],
            intervals: invert_close(iv, i),
        });
    }
    if n == 3 {
        let mut open = invert_close(iv, 0);
        if let Some(top) = open.last_mut() {
            *top += 12;
        }
        out.push(Voicing {
            name: "open (top +8ve)",
            intervals: open,
        });
        let close = invert_close(iv, 0);
        if close.len() == 3 {
            out.push(Voicing {
                name: "spread R–5–10",
                intervals: vec![close[0], close[2], close[1] + 12],
            });
            out.push(Voicing {
                name: "spread R–10–5+8ve",
                intervals: vec![close[0], close[1] + 12, close[2] + 12],
            });
        }
    }
    if n >= 4 {
        for (label, k) in [("drop 2", 2usize), ("drop 3", 3)] {
            if let Some(d) = drop_from_top(iv, k) {
                out.push(Voicing {
                    name: label,
                    intervals: d,
                });
            }
        }
        for i in 1..n {
            let inv = invert_close(iv, i);
            if let Some(d) = drop_from_top(&inv, 2) {
                out.push(Voicing {
                    name: match i {
                        1 => "drop 2 / 1st inv",
                        2 => "drop 2 / 2nd inv",
                        _ => "drop 2 / 3rd inv",
                    },
                    intervals: d,
                });
            }
        }
        let mut shell = iv.to_vec();
        shell.sort_unstable();
        if shell.len() >= 4 {
            out.push(Voicing {
                name: "shell (R–3–7)",
                intervals: vec![shell[0], shell[1], shell[3]],
            });
        }
    }
    out
}

pub fn voicing_midis(root: i32, v: &Voicing) -> Vec<i32> {
    v.intervals.iter().map(|i| root + i).collect()
}

#[derive(Clone)]
pub struct ProgStep {
    pub roman: &'static str,
    pub root: i32,
    pub kind: &'static ChordKind,
}

#[derive(Clone)]
pub struct Progression {
    pub name: &'static str,
    pub hint: &'static str,
    pub steps: Vec<ProgStep>,
}

fn step(tonic: i32, st: i32, slug: &'static str, roman: &'static str) -> ProgStep {
    ProgStep {
        roman,
        root: tonic + st,
        kind: kind_by_slug(slug).unwrap_or(&CHORD_KINDS[0]),
    }
}

/// Common progressions in the key of `tonic` (MIDI of I / i).
pub fn progressions_for(tonic: i32) -> Vec<Progression> {
    vec![
        Progression {
            name: "I – IV – V – I",
            hint: "classic cadence",
            steps: vec![
                step(tonic, 0, "major", "I"),
                step(tonic, 5, "major", "IV"),
                step(tonic, 7, "major", "V"),
                step(tonic, 0, "major", "I"),
            ],
        },
        Progression {
            name: "I – V – vi – IV",
            hint: "pop / axis",
            steps: vec![
                step(tonic, 0, "major", "I"),
                step(tonic, 7, "major", "V"),
                step(tonic, 9, "minor", "vi"),
                step(tonic, 5, "major", "IV"),
            ],
        },
        Progression {
            name: "I – vi – IV – V",
            hint: "50s doo-wop",
            steps: vec![
                step(tonic, 0, "major", "I"),
                step(tonic, 9, "minor", "vi"),
                step(tonic, 5, "major", "IV"),
                step(tonic, 7, "major", "V"),
            ],
        },
        Progression {
            name: "ii – V – I",
            hint: "jazz turnaround",
            steps: vec![
                step(tonic, 2, "min7", "ii7"),
                step(tonic, 7, "7", "V7"),
                step(tonic, 0, "maj7", "IΔ"),
            ],
        },
        Progression {
            name: "I – V – vi – iii – IV – I – IV – V",
            hint: "Pachelbel",
            steps: vec![
                step(tonic, 0, "major", "I"),
                step(tonic, 7, "major", "V"),
                step(tonic, 9, "minor", "vi"),
                step(tonic, 4, "minor", "iii"),
                step(tonic, 5, "major", "IV"),
                step(tonic, 0, "major", "I"),
                step(tonic, 5, "major", "IV"),
                step(tonic, 7, "major", "V"),
            ],
        },
        Progression {
            name: "vi – IV – I – V",
            hint: "sensitive pop",
            steps: vec![
                step(tonic, 9, "minor", "vi"),
                step(tonic, 5, "major", "IV"),
                step(tonic, 0, "major", "I"),
                step(tonic, 7, "major", "V"),
            ],
        },
        Progression {
            name: "I – IV – I – V",
            hint: "folk",
            steps: vec![
                step(tonic, 0, "major", "I"),
                step(tonic, 5, "major", "IV"),
                step(tonic, 0, "major", "I"),
                step(tonic, 7, "major", "V"),
            ],
        },
        Progression {
            name: "12-bar blues",
            hint: "I I I I  IV IV I I  V IV I V",
            steps: vec![
                step(tonic, 0, "7", "I7"),
                step(tonic, 0, "7", "I7"),
                step(tonic, 0, "7", "I7"),
                step(tonic, 0, "7", "I7"),
                step(tonic, 5, "7", "IV7"),
                step(tonic, 5, "7", "IV7"),
                step(tonic, 0, "7", "I7"),
                step(tonic, 0, "7", "I7"),
                step(tonic, 7, "7", "V7"),
                step(tonic, 5, "7", "IV7"),
                step(tonic, 0, "7", "I7"),
                step(tonic, 7, "7", "V7"),
            ],
        },
        Progression {
            name: "i – iv – V – i",
            hint: "minor cadence",
            steps: vec![
                step(tonic, 0, "minor", "i"),
                step(tonic, 5, "minor", "iv"),
                step(tonic, 7, "major", "V"),
                step(tonic, 0, "minor", "i"),
            ],
        },
        Progression {
            name: "i – ♭VII – ♭VI – V",
            hint: "Andalusian",
            steps: vec![
                step(tonic, 0, "minor", "i"),
                step(tonic, 10, "major", "♭VII"),
                step(tonic, 8, "major", "♭VI"),
                step(tonic, 7, "major", "V"),
            ],
        },
        Progression {
            name: "i – ♭VI – ♭III – ♭VII",
            hint: "epic minor",
            steps: vec![
                step(tonic, 0, "minor", "i"),
                step(tonic, 8, "major", "♭VI"),
                step(tonic, 3, "major", "♭III"),
                step(tonic, 10, "major", "♭VII"),
            ],
        },
        Progression {
            name: "I – bVII – IV – I",
            hint: "mixolydian rock",
            steps: vec![
                step(tonic, 0, "major", "I"),
                step(tonic, 10, "major", "♭VII"),
                step(tonic, 5, "major", "IV"),
                step(tonic, 0, "major", "I"),
            ],
        },
    ]
}
