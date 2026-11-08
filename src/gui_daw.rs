//! Step 3: arrange — drop chords on MIDI, stretch audio to the grid.

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::daw::{AudioClip, MidiClip};
use crate::engine::Shared;
use crate::sample::{pick_audio_file, Sample};
use crate::theory::{
    chord_midis, midi_to_name, progressions_for, voicing_midis, voicings_for, CHORD_KINDS,
};

const PANEL: Color32 = Color32::from_rgb(24, 28, 38);
const ACCENT: Color32 = Color32::from_rgb(245, 180, 70);
const MUTED: Color32 = Color32::from_rgb(140, 148, 165);
const OK: Color32 = Color32::from_rgb(90, 200, 140);
const MIDI_COL: Color32 = Color32::from_rgb(70, 50, 16);
const AUD_COL: Color32 = Color32::from_rgb(30, 70, 90);
const RATE: u32 = 48000;
const PX_BEAT: f32 = 36.0;

pub struct DawUi {
    pub holding: Option<(String, Vec<i32>)>,
    pub sel_audio: Option<usize>,
    pub handle: Option<u8>,
}

impl Default for DawUi {
    fn default() -> Self {
        Self {
            holding: None,
            sel_audio: None,
            handle: None,
        }
    }
}

pub fn daw_page(
    ui: &mut egui::Ui,
    shared: &Arc<Shared>,
    daw: &mut DawUi,
    analysis: Option<&crate::engine::Analysis>,
) {
    let playing = shared.daw_play.load(Ordering::Relaxed);
    ui.horizontal(|ui| {
        if ui
            .add(egui::Button::new(if playing { "  Stop  " } else { "  Play  " }).fill(
                if playing {
                    Color32::from_rgb(160, 40, 40)
                } else {
                    Color32::from_rgb(40, 90, 60)
                },
            ))
            .clicked()
        {
            if playing {
                shared.daw_play.store(false, Ordering::Relaxed);
            } else {
                shared.playhead.store(0, Ordering::Relaxed);
                shared.daw_play.store(true, Ordering::Relaxed);
            }
        }
        if let Ok(mut song) = shared.song.lock() {
            ui.label(RichText::new("BPM").size(12.0).color(MUTED));
            ui.add(egui::DragValue::new(&mut song.bpm).range(40.0..=240.0).speed(0.5));
        }
        let head = shared.playhead.load(Ordering::Relaxed) as f32 / RATE as f32;
        ui.label(RichText::new(format!("{head:.2}s")).color(MUTED));
        if ui.button("Load sample").clicked() {
            if let Some(p) = pick_audio_file() {
                match Sample::load(&p) {
                    Ok(s) => {
                        if let Ok(mut g) = shared.err.lock() {
                            *g = None;
                        }
                        if let Ok(mut sm) = shared.sampler.lock() {
                            *sm = Some(s.clone());
                        }
                        if let Ok(mut song) = shared.song.lock() {
                            let beats = s.audible().len() as f32 / s.sr as f32 * song.bpm / 60.0;
                            song.audio.push(AudioClip {
                                start_beats: 0.0,
                                len_beats: beats.max(0.25),
                                sample: std::sync::Arc::new(s),
                                in_norm: 0.0,
                                out_norm: 1.0,
                            });
                        }
                    }
                    Err(e) => {
                        if let Ok(mut g) = shared.err.lock() {
                            *g = Some(e.to_string());
                        }
                    }
                }
            }
        }
        if ui.button("Use recorded take").clicked() {
            if let Ok(buf) = shared.rec_buf.lock() {
                if buf.len() > 1000 {
                    let s = Sample::from_buffer("take".into(), buf.clone(), RATE);
                    if let Ok(mut sm) = shared.sampler.lock() {
                        *sm = Some(s.clone());
                    }
                    if let Ok(mut song) = shared.song.lock() {
                        let beats = s.audible().len() as f32 / s.sr as f32 * song.bpm / 60.0;
                        song.audio.push(AudioClip {
                            start_beats: 0.0,
                            len_beats: beats.max(0.25),
                            sample: std::sync::Arc::new(s),
                            in_norm: 0.0,
                            out_norm: 1.0,
                        });
                    }
                }
            }
        }
    });

    if let Ok(sm) = shared.sampler.lock() {
        if let Some(s) = sm.as_ref() {
            ui.label(
                RichText::new(format!(
                    "Sample “{}”  detected {} (MIDI {})  ·  mapped across the keyboard by pitch-shift",
                    s.name,
                    s.root_name(),
                    s.root_midi
                ))
                .size(13.0)
                .color(OK),
            );
        } else {
            ui.label(
                RichText::new("Load a wav/mp3 as the MIDI instrument (key is auto-detected). Sine fallback until then.")
                    .size(12.0)
                    .color(MUTED),
            );
        }
    }

    ui.add_space(8.0);
    ui.columns(2, |cols| {
        cols[0].vertical(|ui| {
            palette(ui, daw, analysis);
        });
        cols[1].vertical(|ui| {
            mixer(ui, shared);
        });
    });
    ui.add_space(8.0);
    sequencer(ui, shared, daw);
}

fn palette(ui: &mut egui::Ui, daw: &mut DawUi, analysis: Option<&crate::engine::Analysis>) {
    ui.label(RichText::new("Chords — drag onto the MIDI lane").size(14.0).color(ACCENT));
    ui.label(RichText::new("or click a chord, then click the lane").size(11.0).color(MUTED));
    let tonic = analysis.map(|a| a.root).unwrap_or(60);
    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for k in CHORD_KINDS.iter().take(5) {
                let notes = chord_midis(tonic, k);
                let lab = format!("{} {}", midi_to_name(tonic), k.name);
                chord_chip(ui, daw, lab, notes);
            }
        });
        if let Some(a) = analysis {
            for prog in progressions_for(a.root).into_iter().take(4) {
                ui.label(RichText::new(prog.name).size(11.0).color(MUTED));
                ui.horizontal_wrapped(|ui| {
                    for s in &prog.steps {
                        let notes = voicings_for(s.kind)
                            .into_iter()
                            .next()
                            .map(|v| voicing_midis(s.root, &v))
                            .unwrap_or_else(|| chord_midis(s.root, s.kind));
                        let lab = format!("{} {}", s.roman, midi_to_name(s.root));
                        chord_chip(ui, daw, lab, notes);
                    }
                });
            }
        }
    });
    if let Some((lab, _)) = &daw.holding {
        ui.label(RichText::new(format!("holding {lab} — click MIDI lane to drop")).color(ACCENT));
    }
}

fn chord_chip(ui: &mut egui::Ui, daw: &mut DawUi, lab: String, notes: Vec<i32>) {
    let r = ui.add(egui::Button::new(&lab).fill(MIDI_COL));
    if r.dragged() || r.clicked() {
        daw.holding = Some((lab, notes));
    }
}

fn mixer(ui: &mut egui::Ui, shared: &Arc<Shared>) {
    ui.label(RichText::new("Mixer").size(14.0).color(ACCENT));
    if let Ok(mut song) = shared.song.lock() {
        for ch in [&mut song.midi_mix] {
            ui.horizontal(|ui| {
                ui.label(&ch.name);
                ui.add(egui::Slider::new(&mut ch.gain, 0.0..=1.5).show_value(false));
                ui.checkbox(&mut ch.mute, "M");
                ui.checkbox(&mut ch.solo, "S");
            });
        }
        ui.horizontal(|ui| {
            ui.label(&song.audio_mix.name);
            ui.add(egui::Slider::new(&mut song.audio_mix.gain, 0.0..=1.5).show_value(false));
            ui.checkbox(&mut song.audio_mix.mute, "M");
            ui.checkbox(&mut song.audio_mix.solo, "S");
        });
    }
}

fn sequencer(ui: &mut egui::Ui, shared: &Arc<Shared>, daw: &mut DawUi) {
    let bpm = shared.song.lock().map(|s| s.bpm).unwrap_or(120.0);
    let beats = shared.song.lock().map(|s| s.length_beats()).unwrap_or(16.0).max(16.0);
    let w = (beats * PX_BEAT).max(ui.available_width());
    let lane_h = 56.0;

    ui.label(RichText::new("Sequencer  ·  drag clip edges to stretch to the beat  ·  inner handles hide audio (not delete)").size(12.0).color(MUTED));

    // ruler
    let (ruler, _) = ui.allocate_exact_size(Vec2::new(w.min(ui.available_width()), 18.0), Sense::hover());
    ui.painter().rect_filled(ruler, CornerRadius::same(2), Color32::from_rgb(18, 20, 28));
    let vis = ruler.width();
    let beat0 = 0.0;
    let nbeats = (vis / PX_BEAT).ceil() as i32 + 1;
    for b in 0..nbeats {
        let x = ruler.left() + b as f32 * PX_BEAT;
        let bar = b % 4 == 0;
        ui.painter().line_segment(
            [Pos2::new(x, ruler.top()), Pos2::new(x, ruler.bottom())],
            Stroke::new(1.0_f32, if bar { MUTED } else { Color32::from_rgb(40, 44, 54) }),
        );
        if bar {
            ui.painter().text(
                Pos2::new(x + 3.0, ruler.top()),
                egui::Align2::LEFT_TOP,
                format!("{}", b / 4 + 1),
                egui::FontId::proportional(10.0),
                MUTED,
            );
        }
    }
    let head_t = shared.playhead.load(Ordering::Relaxed) as f32 / RATE as f32;
    let head_beat = head_t * bpm / 60.0;
    let hx = ruler.left() + (head_beat - beat0) * PX_BEAT;
    ui.painter().line_segment(
        [Pos2::new(hx, ruler.top()), Pos2::new(hx, ruler.bottom())],
        Stroke::new(1.5_f32, ACCENT),
    );

    midi_lane(ui, shared, daw, vis, lane_h);
    audio_lane(ui, shared, daw, vis, lane_h + 16.0);
}

fn midi_lane(ui: &mut egui::Ui, shared: &Arc<Shared>, daw: &mut DawUi, w: f32, h: f32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("MIDI").size(12.0).color(MUTED));
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w - 8.0, h), Sense::click_and_drag());
        ui.painter().rect_filled(rect, CornerRadius::same(6), Color32::from_rgb(16, 18, 24));
        grid(ui, rect);
        if let Ok(song) = shared.song.lock() {
            for c in &song.midi {
                let x = rect.left() + c.start_beats * PX_BEAT;
                let ww = (c.len_beats * PX_BEAT).max(12.0);
                let r = Rect::from_min_size(Pos2::new(x, rect.top() + 8.0), Vec2::new(ww, h - 16.0));
                ui.painter().rect_filled(r, CornerRadius::same(4), MIDI_COL);
                ui.painter().text(
                    r.left_top() + Vec2::new(6.0, 8.0),
                    egui::Align2::LEFT_TOP,
                    &c.label,
                    egui::FontId::proportional(12.0),
                    ACCENT,
                );
            }
        }
        if resp.clicked() {
            if let Some((lab, notes)) = daw.holding.take() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let beat = ((p.x - rect.left()) / PX_BEAT).max(0.0);
                    let beat = (beat * 4.0).round() / 4.0;
                    if let Ok(mut song) = shared.song.lock() {
                        song.midi.push(MidiClip {
                            start_beats: beat,
                            len_beats: 1.0,
                            label: lab,
                            notes,
                        });
                    }
                }
            }
        }
    });
}

fn audio_lane(ui: &mut egui::Ui, shared: &Arc<Shared>, daw: &mut DawUi, w: f32, h: f32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Audio").size(12.0).color(MUTED));
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w - 8.0, h), Sense::click_and_drag());
        ui.painter().rect_filled(rect, CornerRadius::same(6), Color32::from_rgb(16, 18, 24));
        grid(ui, rect);
        let mut stretch: Option<(usize, f32)> = None;
        if let Ok(song) = shared.song.lock() {
            for (i, c) in song.audio.iter().enumerate() {
                let x = rect.left() + c.start_beats * PX_BEAT;
                let ww = (c.len_beats * PX_BEAT).max(16.0);
                let r = Rect::from_min_size(Pos2::new(x, rect.top() + 6.0), Vec2::new(ww, h - 12.0));
                ui.painter().rect_filled(r, CornerRadius::same(4), AUD_COL);
                // hidden (not deleted) region shading inside clip uses in/out of sample
                let x0 = r.left() + c.in_norm * r.width();
                let x1 = r.left() + c.out_norm * r.width();
                ui.painter().rect_filled(
                    Rect::from_min_max(r.min, Pos2::new(x0, r.bottom())),
                    0.0,
                    Color32::from_rgba_unmultiplied(0, 0, 0, 90),
                );
                ui.painter().rect_filled(
                    Rect::from_min_max(Pos2::new(x1, r.top()), r.max),
                    0.0,
                    Color32::from_rgba_unmultiplied(0, 0, 0, 90),
                );
                ui.painter().line_segment(
                    [Pos2::new(x0, r.top()), Pos2::new(x0, r.bottom())],
                    Stroke::new(2.0_f32, ACCENT),
                );
                ui.painter().line_segment(
                    [Pos2::new(x1, r.top()), Pos2::new(x1, r.bottom())],
                    Stroke::new(2.0_f32, ACCENT),
                );
                ui.painter().text(
                    r.left_top() + Vec2::new(8.0, 8.0),
                    egui::Align2::LEFT_TOP,
                    &c.sample.name,
                    egui::FontId::proportional(11.0),
                    Color32::WHITE,
                );
                let edge = Rect::from_min_size(Pos2::new(r.right() - 8.0, r.top()), Vec2::new(8.0, r.height()));
                ui.painter().rect_filled(edge, 0.0, Color32::from_rgb(180, 220, 255));
                if resp.clicked() {
                    daw.sel_audio = Some(i);
                }
                if resp.dragged() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        if daw.handle.is_none() {
                            let d0 = (p.x - x0).abs();
                            let d1 = (p.x - x1).abs();
                            let de = (p.x - r.right()).abs();
                            daw.handle = Some(if de < d0.min(d1) { 2 } else if d0 <= d1 { 0 } else { 1 });
                            daw.sel_audio = Some(i);
                        }
                        stretch = Some((i, p.x));
                    }
                }
            }
        }
        if resp.drag_stopped() {
            daw.handle = None;
        }
        if let Some((i, px)) = stretch {
            if let Ok(mut song) = shared.song.lock() {
                if let Some(c) = song.audio.get_mut(i) {
                    let x = rect.left() + c.start_beats * PX_BEAT;
                    let ww = (c.len_beats * PX_BEAT).max(16.0);
                    match daw.handle {
                        Some(0) => {
                            c.in_norm = ((px - x) / ww).clamp(0.0, c.out_norm - 0.02);
                        }
                        Some(1) => {
                            c.out_norm = ((px - x) / ww).clamp(c.in_norm + 0.02, 1.0);
                        }
                        Some(2) => {
                            let beats = ((px - x) / PX_BEAT).max(0.25);
                            c.len_beats = (beats * 4.0).round() / 4.0;
                        }
                        _ => {}
                    }
                }
            }
        }
    });
}

fn grid(ui: &mut egui::Ui, rect: Rect) {
    let n = (rect.width() / PX_BEAT).ceil() as i32;
    for b in 0..=n {
        let x = rect.left() + b as f32 * PX_BEAT;
        ui.painter().line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(
                1.0_f32,
                if b % 4 == 0 {
                    Color32::from_rgb(50, 54, 64)
                } else {
                    Color32::from_rgb(28, 32, 40)
                },
            ),
        );
    }
}
