//! Record a hum, then show spectrogram + mapped chords (not live).

use anyhow::Result;
use eframe::egui::{
    self, Color32, ColorImage, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke,
    TextureHandle, TextureOptions, Vec2,
};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::engine::{self, Analysis, Shared, F_MAX, SPEC_BINS};
use crate::theory::{
    chord_midis, kind_by_slug, midi_to_freq, midi_to_name, progressions_for, voicing_midis,
    voicings_for, ChordKind, CHORD_KINDS,
};

const BG: Color32 = Color32::from_rgb(14, 16, 22);
const PANEL: Color32 = Color32::from_rgb(24, 28, 38);
const ACCENT: Color32 = Color32::from_rgb(245, 180, 70);
const MUTED: Color32 = Color32::from_rgb(140, 148, 165);
const OK: Color32 = Color32::from_rgb(90, 200, 140);
const PITCH: Color32 = Color32::from_rgb(120, 220, 255);
const RATE: u32 = 48000;

pub fn run(_play: bool, kind: usize, rate: u32, threshold: f32, gate: f32) -> Result<()> {
    let shared = Shared::new(false, kind);
    let _h = engine::spawn(shared.clone(), rate, threshold, gate);
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 760.0])
            .with_min_inner_size([760.0, 600.0])
            .with_title("Humchord"),
        ..Default::default()
    };
    eframe::run_native(
        "Humchord",
        native,
        Box::new(move |_cc| {
            Ok(Box::new(HumUi {
                shared,
                kind,
                spec_tex: None,
                rec_player: None,
                clip_lo: 0.0,
                clip_hi: 1.0,
                last_dur: 0.0,
                drag: None,
                page: 0,
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

struct HumUi {
    shared: Arc<Shared>,
    kind: usize,
    spec_tex: Option<TextureHandle>,
    rec_player: Option<Child>,
    clip_lo: f32,
    clip_hi: f32,
    last_dur: f32,
    drag: Option<u8>,
    page: u8,
}

impl eframe::App for HumUi {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if self.shared.recording.load(Ordering::Relaxed) {
            self.shared.recording.store(false, Ordering::Relaxed);
            self.shared.save_req.store(true, Ordering::SeqCst);
        }
        if let Some(mut c) = self.rec_player.take() {
            let _ = c.kill();
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(50));
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = BG;
        visuals.window_fill = PANEL;
        visuals.override_text_color = Some(Color32::from_rgb(230, 234, 242));
        ctx.set_visuals(visuals);

        let ci1 = self.shared.ci1.load(Ordering::Relaxed);
        let err = self.shared.err.lock().ok().and_then(|g| g.clone());
        let mut recording = self.shared.recording.load(Ordering::Relaxed);
        let rec_frames = self.shared.rec_frames.load(Ordering::Relaxed);
        let rec_path = self.shared.rec_path.lock().ok().and_then(|g| g.clone());
        let analysis = self.shared.analysis.lock().ok().and_then(|g| g.clone());

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BG).inner_margin(18.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("HUMCHORD").size(18.0).color(ACCENT).strong());
                    ui.add_space(12.0);
                    tab_btn(ui, &mut self.page, 0, "Analyze");
                    tab_btn(ui, &mut self.page, 1, "Progressions");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(if ci1 { "CI1 ready" } else { "default input" })
                                .size(13.0)
                                .color(if ci1 { OK } else { MUTED }),
                        );
                    });
                });
                ui.add_space(10.0);
                if let Some(e) = &err {
                    ui.colored_label(Color32::from_rgb(255, 110, 110), e);
                }

                if self.page == 1 {
                    progressions_page(ui, analysis.as_ref(), &self.shared);
                } else {
                ui.horizontal(|ui| {
                    let rec_col = if recording {
                        Color32::from_rgb(160, 40, 40)
                    } else {
                        Color32::from_rgb(70, 50, 16)
                    };
                    if ui
                        .add(
                            egui::Button::new(if recording {
                                "  STOP  "
                            } else {
                                "  RECORD  "
                            })
                            .fill(rec_col),
                        )
                        .clicked()
                    {
                        if recording {
                            recording = false;
                            self.shared.save_req.store(true, Ordering::SeqCst);
                        } else {
                            recording = true;
                            self.spec_tex = None;
                            self.clip_lo = 0.0;
                            self.clip_hi = 1.0;
                            self.last_dur = 0.0;
                        }
                    }
                    let secs = rec_frames as f32 / RATE as f32;
                    ui.label(
                        RichText::new(if recording {
                            format!("●  {secs:.1}s  humming…")
                        } else {
                            "hum a steady note, then Stop".into()
                        })
                        .size(14.0)
                        .color(if recording {
                            Color32::from_rgb(255, 90, 90)
                        } else {
                            MUTED
                        }),
                    );
                    if let Some(p) = rec_path.as_deref() {
                        if ui.button("Play take").clicked() {
                            if let Some(mut c) = self.rec_player.take() {
                                let _ = c.kill();
                            }
                            self.rec_player = Command::new("pw-cat")
                                .args(["--playback", p])
                                .stdin(Stdio::null())
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .spawn()
                                .ok();
                        }
                    }
                });

                ui.add_space(12.0);

                if let Some(a) = analysis.as_ref() {
                    if (a.duration - self.last_dur).abs() > 0.05 {
                        self.clip_lo = 0.0;
                        self.clip_hi = 1.0;
                        self.last_dur = a.duration;
                        self.spec_tex = None;
                    }
                    ui.label(
                        RichText::new(format!(
                            "Detected root  {}   MIDI {}   {:.1} s",
                            midi_to_name(a.root),
                            a.root,
                            a.duration
                        ))
                        .size(20.0)
                        .color(ACCENT),
                    );
                    ui.label(
                        RichText::new(
                            "Spectrogram of your take  ·  dots = found notes  ·  cyan = hummed pitch  ·  gold = selected chord",
                        )
                        .size(12.0)
                        .color(MUTED),
                    );
                    ui.add_space(6.0);
                    recorded_spec(ui, ctx, &mut self.spec_tex, a, self.kind, self.clip_lo, self.clip_hi);
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Drag the handles to clip · then Apply clip")
                            .size(12.0)
                            .color(MUTED),
                    );
                    clip_wave(
                        ui,
                        &a.wave,
                        &mut self.clip_lo,
                        &mut self.clip_hi,
                        &mut self.drag,
                    );
                    ui.horizontal(|ui| {
                        let t0 = self.clip_lo * a.duration;
                        let t1 = self.clip_hi * a.duration;
                        ui.label(
                            RichText::new(format!("in {t0:.2}s   out {t1:.2}s   ({:.2}s)", t1 - t0))
                                .size(13.0)
                                .color(MUTED),
                        );
                        if ui
                            .add(egui::Button::new("  Apply clip  ").fill(Color32::from_rgb(70, 50, 16)))
                            .clicked()
                        {
                            self.shared.apply_clip(self.clip_lo, self.clip_hi, RATE);
                            self.spec_tex = None;
                            self.clip_lo = 0.0;
                            self.clip_hi = 1.0;
                        }
                        if ui.button("Reset handles").clicked() {
                            self.clip_lo = 0.0;
                            self.clip_hi = 1.0;
                        }
                    });
                    ui.add_space(10.0);
                    ui.columns(2, |cols| {
                        cols[0].vertical(|ui| {
                            ui.label(RichText::new("Notes the spectrogram found").size(14.0).color(ACCENT));
                            ui.label(RichText::new("every peak mapped to 12-TET").size(11.0).color(MUTED));
                            let max_s = a.notes.first().map(|n| n.strength).unwrap_or(1.0).max(0.001);
                            for n in &a.notes {
                                let bar = ((n.strength / max_s) * 14.0) as usize;
                                ui.label(format!(
                                    "{}   MIDI {:>3}   {:>7.1} Hz   {}",
                                    n.name,
                                    n.midi,
                                    n.hz,
                                    "█".repeat(bar.max(1))
                                ));
                            }
                        });
                        cols[1].vertical(|ui| {
                            ui.label(
                                RichText::new("All voicings — click to hear")
                                    .size(14.0)
                                    .color(ACCENT),
                            );
                            ui.add_space(4.0);
                            egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                                for (i, k) in CHORD_KINDS.iter().enumerate() {
                                    ui.label(RichText::new(format!("Tonic {}", k.name)).size(13.0).color(MUTED));
                                    for v in voicings_for(k) {
                                        let notes = voicing_midis(a.root, &v);
                                        let names = notes.iter().map(|m| midi_to_name(*m)).collect::<Vec<_>>().join("  ");
                                        let ids = notes.iter().map(|m| m.to_string()).collect::<Vec<_>>().join(" ");
                                        if ui
                                            .add(
                                                egui::Button::new(format!(
                                                    "{}    {names}    MIDI {ids}",
                                                    v.name
                                                ))
                                                .fill(Color32::from_rgb(32, 36, 48)),
                                            )
                                            .clicked()
                                        {
                                            self.kind = i;
                                            self.shared.play_notes(notes, RATE * 3 / 2);
                                        }
                                    }
                                    ui.add_space(6.0);
                                }
                                ui.label(RichText::new("Related roots (close voicing)").size(13.0).color(MUTED));
                                for (label, r, k) in extra_harmonies(a.root) {
                                    let idx = CHORD_KINDS.iter().position(|x| x.slug == k.slug).unwrap_or(0);
                                    chord_btn(ui, label, r, k, idx, &mut self.kind, &self.shared);
                                }
                            });
                        });
                    });
                } else if !recording {
                    ui.add_space(40.0);
                    ui.label(
                        RichText::new("Nothing analyzed yet.\nRecord a hum on the CI1 / default mic, then Stop.")
                            .size(16.0)
                            .color(MUTED),
                    );
                }
                }
            });

        self.shared.recording.store(recording, Ordering::Relaxed);
    }
}

fn tab_btn(ui: &mut egui::Ui, page: &mut u8, id: u8, label: &str) {
    let on = *page == id;
    if ui
        .add(
            egui::Button::new(label).fill(if on {
                Color32::from_rgb(70, 50, 16)
            } else {
                Color32::from_rgb(36, 40, 52)
            }),
        )
        .clicked()
    {
        *page = id;
    }
}

fn progressions_page(ui: &mut egui::Ui, analysis: Option<&Analysis>, shared: &Arc<Shared>) {
    let Some(a) = analysis else {
        ui.add_space(30.0);
        ui.label(
            RichText::new("Record and analyze a hum on the Analyze tab first.\nProgressions are built from that root.")
                .size(16.0)
                .color(MUTED),
        );
        return;
    };
    let tonic = a.root;
    let playing = shared.prog_step.load(Ordering::Relaxed);
    ui.label(
        RichText::new(format!(
            "Key of {}  (MIDI {})  ·  click Play on a progression",
            midi_to_name(tonic),
            tonic
        ))
        .size(18.0)
        .color(ACCENT),
    );
    ui.horizontal(|ui| {
        if ui.button("Stop").clicked() {
            shared.stop_progression();
        }
        if playing >= 0 {
            ui.label(RichText::new(format!("playing chord {}", playing + 1)).color(OK));
        }
    });
    ui.add_space(8.0);
    egui::ScrollArea::vertical().show(ui, |ui| {
        for prog in progressions_for(tonic) {
            egui::Frame::new()
                .fill(PANEL)
                .corner_radius(CornerRadius::same(10))
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(prog.name).size(16.0).strong());
                            ui.label(RichText::new(prog.hint).size(12.0).color(MUTED));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(egui::Button::new("  Play  ").fill(Color32::from_rgb(40, 90, 60)))
                                .clicked()
                            {
                                let steps: Vec<Vec<i32>> = prog
                                    .steps
                                    .iter()
                                    .map(|s| {
                                        voicings_for(s.kind)
                                            .into_iter()
                                            .next()
                                            .map(|v| voicing_midis(s.root, &v))
                                            .unwrap_or_else(|| chord_midis(s.root, s.kind))
                                    })
                                    .collect();
                                shared.play_progression(&steps, RATE * 7 / 10);
                            }
                        });
                    });
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        for (i, s) in prog.steps.iter().enumerate() {
                            let on = playing == i as i32;
                            if ui
                                .add(
                                    egui::Button::new(format!(
                                        "{}  {} {}",
                                        s.roman,
                                        midi_to_name(s.root),
                                        s.kind.name
                                    ))
                                    .fill(if on {
                                        Color32::from_rgb(70, 50, 16)
                                    } else {
                                        Color32::from_rgb(36, 40, 52)
                                    }),
                                )
                                .clicked()
                            {
                                shared.play_notes(chord_midis(s.root, s.kind), RATE * 3 / 2);
                            }
                        }
                    });
                });
            ui.add_space(8.0);
        }
    });
}

fn extra_harmonies(root: i32) -> Vec<(&'static str, i32, &'static ChordKind)> {
    let maj = kind_by_slug("major").unwrap();
    let min = kind_by_slug("minor").unwrap();
    let sev = kind_by_slug("7").unwrap();
    vec![
        ("IV", root + 5, maj),
        ("V", root + 7, maj),
        ("V7", root + 7, sev),
        ("vi", root - 3, min),
        ("ii", root + 2, min),
        ("iii", root + 4, min),
    ]
}

fn chord_btn(
    ui: &mut egui::Ui,
    label: &str,
    root: i32,
    k: &ChordKind,
    idx: usize,
    kind: &mut usize,
    shared: &Arc<Shared>,
) {
    let notes = chord_midis(root, k);
    let names: String = notes.iter().map(|m| midi_to_name(*m)).collect::<Vec<_>>().join("  ");
    let midis: String = notes.iter().map(|m| m.to_string()).collect::<Vec<_>>().join(" ");
    let on = *kind == idx && shared.preview_root.load(Ordering::Relaxed) == root;
    let fill = if on {
        Color32::from_rgb(70, 50, 16)
    } else {
        Color32::from_rgb(32, 36, 48)
    };
    if ui
        .add(egui::Button::new(format!("{label}  {}    {names}    MIDI {midis}", k.name)).fill(fill))
        .clicked()
    {
        *kind = idx;
        shared.play_notes(notes, RATE * 3 / 2);
    }
}

fn recorded_spec(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    tex: &mut Option<TextureHandle>,
    a: &Analysis,
    kind: usize,
    lo: f32,
    hi: f32,
) {
    let h = 260.0;
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(8), Color32::from_rgb(8, 10, 14));
    if a.spec.is_empty() {
        return;
    }
    let cols = a.spec.len().max(1);
    let mut img = ColorImage::new([cols, SPEC_BINS], Color32::BLACK);
    for (x, col) in a.spec.iter().enumerate() {
        for y in 0..SPEC_BINS {
            let v = col.get(y).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            let gy = SPEC_BINS - 1 - y;
            img[(x, gy)] = heat(v);
        }
    }
    let t = tex.get_or_insert_with(|| ctx.load_texture("spec", img.clone(), TextureOptions::LINEAR));
    t.set(img, TextureOptions::LINEAR);
    ui.painter().image(
        t.id(),
        rect,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );
    shade_outside(ui, rect, lo, hi);

    let cols = a.spec.len().max(1) as f32;
    for n in &a.notes {
        if n.hz <= 0.0 || n.hz > F_MAX {
            continue;
        }
        let y = rect.bottom() - (n.hz / F_MAX) * rect.height();
        ui.painter().line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            Stroke::new(
                1.0_f32,
                Color32::from_rgba_unmultiplied(180, 220, 255, 70),
            ),
        );
        ui.painter().text(
            Pos2::new(rect.left() + 6.0, y - 1.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{} {}", n.name, n.midi),
            FontId::proportional(10.0),
            Color32::from_rgb(170, 210, 240),
        );
    }
    let step = (a.peaks.len() / 400).max(1);
    for (i, (t, hz)) in a.peaks.iter().enumerate() {
        if i % step != 0 {
            continue;
        }
        if *hz <= 0.0 || *hz > F_MAX {
            continue;
        }
        let x = rect.left() + (*t as f32 / (cols - 1.0).max(1.0)) * rect.width();
        let y = rect.bottom() - (*hz / F_MAX) * rect.height();
        ui.painter().circle_filled(Pos2::new(x, y), 1.6, Color32::from_rgb(220, 240, 255));
    }

    let k = &CHORD_KINDS[kind.min(CHORD_KINDS.len() - 1)];
    for m in chord_midis(a.root, k) {
        let hz = midi_to_freq(m as f32);
        if hz <= 0.0 || hz > F_MAX {
            continue;
        }
        let y = rect.bottom() - (hz / F_MAX) * rect.height();
        ui.painter().line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            Stroke::new(1.4_f32, ACCENT),
        );
        ui.painter().text(
            Pos2::new(rect.right() - 8.0, y - 2.0),
            egui::Align2::RIGHT_BOTTOM,
            format!("{}  MIDI {m}", midi_to_name(m)),
            FontId::proportional(11.0),
            ACCENT,
        );
    }

    if a.f0.len() >= 2 {
        let mut pts = Vec::new();
        for (i, f) in a.f0.iter().enumerate() {
            if let Some(hz) = *f {
                if hz > 0.0 && hz < F_MAX {
                    let x = rect.left() + (i as f32 / (a.f0.len() - 1) as f32) * rect.width();
                    let y = rect.bottom() - (hz / F_MAX) * rect.height();
                    pts.push(Pos2::new(x, y));
                }
            }
        }
        if pts.len() >= 2 {
            ui.painter()
                .add(egui::Shape::line(pts, Stroke::new(2.0_f32, PITCH)));
        }
    }

    ui.painter().text(
        Pos2::new(rect.left() + 6.0, rect.top() + 4.0),
        egui::Align2::LEFT_TOP,
        "2 kHz",
        FontId::proportional(11.0),
        MUTED,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 6.0, rect.bottom() - 14.0),
        egui::Align2::LEFT_TOP,
        "0 Hz",
        FontId::proportional(11.0),
        MUTED,
    );
}

fn heat(v: f32) -> Color32 {
    let v = v.clamp(0.0, 1.0);
    if v < 0.33 {
        let t = v / 0.33;
        Color32::from_rgb(0, 0, (t * 90.0) as u8)
    } else if v < 0.66 {
        let t = (v - 0.33) / 0.33;
        Color32::from_rgb((t * 50.0) as u8, (t * 170.0) as u8, 190)
    } else {
        let t = (v - 0.66) / 0.34;
        Color32::from_rgb((90.0 + t * 165.0) as u8, (190.0 + t * 40.0) as u8, (90.0 - t * 80.0) as u8)
    }
}

fn shade_outside(ui: &mut egui::Ui, rect: Rect, lo: f32, hi: f32) {
    let dim = Color32::from_rgba_unmultiplied(8, 10, 14, 160);
    let x0 = rect.left() + lo.clamp(0.0, 1.0) * rect.width();
    let x1 = rect.left() + hi.clamp(0.0, 1.0) * rect.width();
    ui.painter().rect_filled(
        Rect::from_min_max(rect.min, Pos2::new(x0, rect.bottom())),
        0.0,
        dim,
    );
    ui.painter().rect_filled(
        Rect::from_min_max(Pos2::new(x1, rect.top()), rect.max),
        0.0,
        dim,
    );
}

fn clip_wave(
    ui: &mut egui::Ui,
    wave: &[f32],
    lo: &mut f32,
    hi: &mut f32,
    drag: &mut Option<u8>,
) {
    let h = 64.0;
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click_and_drag());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(8), Color32::from_rgb(12, 14, 20));
    if wave.len() >= 2 {
        let n = wave.len() as f32;
        let mut pts = Vec::with_capacity(wave.len());
        for (i, s) in wave.iter().enumerate() {
            let x = rect.left() + (i as f32 / (n - 1.0)) * rect.width();
            let y = rect.center().y - s.clamp(-1.0, 1.0) * (h * 0.42);
            pts.push(Pos2::new(x, y));
        }
        ui.painter().add(egui::Shape::line(
            pts,
            Stroke::new(1.4_f32, Color32::from_rgb(80, 170, 255)),
        ));
    }
    shade_outside(ui, rect, *lo, *hi);
    let x_lo = rect.left() + *lo * rect.width();
    let x_hi = rect.left() + *hi * rect.width();
    let handle = |x: f32, color: Color32| {
        ui.painter().line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(2.0_f32, color),
        );
        ui.painter().rect_filled(
            Rect::from_center_size(Pos2::new(x, rect.center().y), Vec2::new(8.0, 18.0)),
            CornerRadius::same(2),
            color,
        );
    };
    handle(x_lo, ACCENT);
    handle(x_hi, ACCENT);

    if resp.drag_started() {
        if let Some(p) = resp.interact_pointer_pos() {
            let t = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            *drag = Some(if (t - *lo).abs() <= (t - *hi).abs() { 0 } else { 1 });
        }
    }
    if resp.dragged() {
        if let Some(p) = resp.interact_pointer_pos() {
            let t = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            match *drag {
                Some(0) => *lo = t.min(*hi - 0.02),
                Some(1) => *hi = t.max(*lo + 0.02),
                _ => {}
            }
        }
    }
    if resp.drag_stopped() {
        *drag = None;
    }
}
