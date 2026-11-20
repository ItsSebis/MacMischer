//! Console views: channel strip, meter bridge, detail panel.

use crate::actions::Action;
use crate::theme;
use crate::widgets::{self, Ballistics};
use eframe::egui::{self, Align2, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::collections::HashMap;
use xair_core::{Meters, MixerState};
use xair_proto::{fader, Param, SendTo, StripId, Value};

pub const STRIP_W: f32 = 86.0;

/// Everything a view needs for one frame (read-only) plus the action sink.
pub struct Ctx<'a> {
    pub st: &'a MixerState,
    pub meters: &'a Meters,
    pub ball: &'a mut HashMap<StripId, Ballistics>,
    pub selected: StripId,
    pub actions: &'a mut Vec<Action>,
}

impl Ctx<'_> {
    fn meter(&mut self, s: StripId) -> (f32, f32) {
        let raw = self.meters.strip(s).unwrap_or(-128.0);
        self.ball.entry(s).or_default().update(raw)
    }
}

/// Name tag with strip colour; click selects.
fn name_tag(ui: &mut Ui, c: &mut Ctx, s: StripId, width: f32) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::click());
    let col = theme::strip_color(c.st.color(s));
    let inverted = c.st.color(s) >= 8;
    let p = ui.painter();
    let (bg, fg) = if inverted { (col, egui::Color32::BLACK) } else { (theme::BG, col) };
    p.rect_filled(rect, 3.0, bg);
    p.rect_stroke(rect, 3.0, Stroke::new(1.5, col), StrokeKind::Inside);
    let mut name = c.st.name(s);
    if name.chars().count() > 10 {
        name = name.chars().take(10).collect();
    }
    p.text(rect.center(), Align2::CENTER_CENTER, name, FontId::proportional(12.5), fg);
    if resp.clicked() {
        c.actions.push(Action::Select(s));
    }
}

/// One channel strip. `send` = bank is bound to a bus: fader controls that send.
pub fn strip(ui: &mut Ui, c: &mut Ctx, s: StripId, height: f32, send: Option<SendTo>) {
    let sel = c.selected == s;
    let send = send.filter(|_| s.has_sends());
    let (frame_rect, _) = ui.allocate_exact_size(Vec2::new(STRIP_W, height), Sense::hover());
    let bg = if sel { theme::STRIP_SEL } else { theme::STRIP };
    ui.painter().rect_filled(frame_rect, 4.0, bg);
    if sel {
        ui.painter().rect_stroke(frame_rect, 4.0, Stroke::new(1.5, theme::ACCENT), StrokeKind::Inside);
    }
    let accent = match send {
        Some(t) => theme::strip_color(c.st.color(t.strip())),
        None => theme::strip_color(c.st.color(s)),
    };
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(frame_rect.shrink(4.0))
            .layout(egui::Layout::top_down(egui::Align::Center)),
    );
    let ui = &mut child;
    ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
    let w = ui.available_width();

    name_tag(ui, c, s, w);
    let (param, value) = match send {
        Some(t) => (Param::SendLevel(s, t), c.st.send_level(s, t)),
        None => (Param::Fader(s), c.st.fader(s)),
    };
    let label = widgets::fmt_db(fader::to_db(value));
    ui.vertical_centered(|ui| {
        let txt = egui::RichText::new(label).monospace().size(13.0);
        ui.label(if send.is_some() { txt.color(accent) } else { txt });
    });

    // meter + fader
    let bottom_h = 26.0 + 18.0 + 8.0;
    let fh = (ui.available_height() - bottom_h).max(80.0);
    ui.horizontal(|ui| {
        let (mrect, mresp) = ui.allocate_exact_size(Vec2::new(10.0, fh), Sense::click());
        let (lvl, peak) = c.meter(s);
        if !matches!(s, StripId::Dca(_)) {
            if let Some((l, r)) = c.meters.stereo(s) {
                let half = Rect::from_min_size(mrect.min, Vec2::new(4.5, fh));
                widgets::paint_meter(ui, half, l, l);
                widgets::paint_meter(ui, half.translate(Vec2::new(5.5, 0.0)), r, r);
            } else {
                widgets::paint_meter(ui, mrect, lvl, peak);
            }
        }
        if mresp.clicked() {
            c.actions.push(Action::Select(s));
        }
        let id = egui::Id::unique(("fader", s, send));
        if let Some(v) = widgets::fader(ui, Vec2::new(w - 14.0, fh), value, accent, id) {
            c.actions.push(Action::Set(param, Value::Float(v)));
            c.actions.push(Action::Select(s));
        }
    });

    if s.has_pan() && send.is_none() {
        let id = egui::Id::unique(("pan", s));
        if let Some(v) = widgets::hbar(ui, Vec2::new(w, 10.0), c.st.pan(s), true, id) {
            c.actions.push(Action::Set(Param::Pan(s), Value::Float(v)));
        }
    } else {
        ui.add_space(14.0);
    }
    ui.horizontal(|ui| {
        let bw = (w - 4.0) / 2.0;
        let muted = !c.st.on(s);
        if widgets::toggle(ui, Vec2::new(bw, 22.0), "M", muted, theme::MUTE).clicked() {
            c.actions.push(Action::Set(Param::On(s), Value::Int(muted as i32)));
        }
        let solo = Param::Solo(s.solo_index());
        let soloed = c.st.get(solo).and_then(Value::as_i32).unwrap_or(0) != 0;
        if widgets::toggle(ui, Vec2::new(bw, 22.0), "S", soloed, theme::SOLO).clicked() {
            c.actions.push(Action::Set(solo, Value::Int(!soloed as i32)));
        }
    });
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(s.short()).size(11.0).color(theme::DIM));
    });
}

/// Compact always-visible meters for every strip (except DCAs).
pub fn meter_bridge(ui: &mut Ui, c: &mut Ctx) {
    let strips: Vec<StripId> = StripId::all().into_iter().filter(|s| !matches!(s, StripId::Dca(_))).collect();
    let h = ui.available_height();
    let gap = 3.0;
    let w = ((ui.available_width() - gap * strips.len() as f32) / strips.len() as f32).clamp(10.0, 34.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (i, s) in strips.into_iter().enumerate() {
            if i == 16 || i == 21 || i == 27 || i == 31 {
                ui.add_space(6.0); // group gaps: inputs | aux+fx | buses | fx sends | main
            }
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
            let mrect = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.max.y - 14.0));
            let (lvl, peak) = c.meter(s);
            widgets::paint_meter(ui, mrect, lvl, peak);
            if !c.st.on(s) {
                ui.painter().rect_stroke(mrect, 2.0, Stroke::new(1.0, theme::MUTE), StrokeKind::Inside);
            }
            let col = if c.selected == s { theme::ACCENT } else { theme::strip_color(c.st.color(s)) };
            ui.painter().text(
                Pos2::new(rect.center().x, rect.bottom() - 6.0),
                Align2::CENTER_CENTER,
                s.short(),
                FontId::proportional(9.5),
                col,
            );
            if resp.clicked() {
                c.actions.push(Action::Select(s));
            }
            resp.on_hover_text(c.st.name(s));
        }
    });
}

/// Headamp gain in dB: mic inputs −12..+60, aux line input −12..+20.
fn gain_db(headamp: u8, v: f32) -> f32 {
    if headamp <= 16 { v * 72.0 - 12.0 } else { v * 32.0 - 12.0 }
}

/// Detail panel for the selected strip.
pub fn detail(ui: &mut Ui, c: &mut Ctx) {
    let s = c.selected;
    ui.horizontal(|ui| {
        // Identity + level
        ui.vertical(|ui| {
            ui.set_width(200.0);
            name_tag(ui, c, s, 200.0);
            ui.label(egui::RichText::new(s.to_string()).color(theme::DIM));
            let db = c.st.fader_db(s);
            ui.label(egui::RichText::new(format!("{} dB", widgets::fmt_db(db))).size(30.0).monospace());
            ui.horizontal(|ui| {
                let muted = !c.st.on(s);
                if widgets::toggle(ui, Vec2::new(64.0, 28.0), "MUTE", muted, theme::MUTE).clicked() {
                    c.actions.push(Action::Set(Param::On(s), Value::Int(muted as i32)));
                }
                let solo = Param::Solo(s.solo_index());
                let soloed = c.st.get(solo).and_then(Value::as_i32).unwrap_or(0) != 0;
                if widgets::toggle(ui, Vec2::new(64.0, 28.0), "SOLO", soloed, theme::SOLO).clicked() {
                    c.actions.push(Action::Set(solo, Value::Int(!soloed as i32)));
                }
            });
        });
        ui.separator();

        // Input section
        ui.vertical(|ui| {
            ui.set_width(220.0);
            section(ui, "INPUT");
            if let Some(h) = s.headamp() {
                let g = c.st.gain(h);
                ui.label(format!("Gain  {:+.1} dB", gain_db(h, g)));
                if let Some(v) = widgets::hbar(ui, Vec2::new(200.0, 14.0), g, false, egui::Id::unique(("gain", h))) {
                    c.actions.push(Action::Set(Param::Gain(h), Value::Float(v)));
                }
                if h <= 16 {
                    let ph = c.st.phantom(h);
                    if widgets::toggle(ui, Vec2::new(64.0, 22.0), "+48V", ph, theme::PHANTOM).clicked() {
                        c.actions.push(Action::Set(Param::Phantom(h), Value::Int(!ph as i32)));
                    }
                }
            } else {
                ui.label(egui::RichText::new("no preamp").color(theme::DIM));
            }
            if s.has_pan() {
                ui.add_space(6.0);
                ui.label(format!("Pan  {}", widgets::fmt_pan(c.st.pan(s))));
                if let Some(v) = widgets::hbar(ui, Vec2::new(200.0, 14.0), c.st.pan(s), true, egui::Id::unique("dpan")) {
                    c.actions.push(Action::Set(Param::Pan(s), Value::Float(v)));
                }
            }
        });
        ui.separator();

        // Sends
        ui.vertical(|ui| {
            section(ui, "SENDS");
            if !s.has_sends() {
                ui.label(egui::RichText::new("This strip has no sends.").color(theme::DIM));
                return;
            }
            ui.horizontal_top(|ui| {
                let cols: [Vec<SendTo>; 2] = [
                    SendTo::all().filter(|t| matches!(t, SendTo::Bus(_))).collect(),
                    SendTo::all().filter(|t| matches!(t, SendTo::Fx(_))).collect(),
                ];
                for (ci, col) in cols.into_iter().enumerate() {
                    egui::Grid::new(("sends", ci)).num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
                        for t in col {
                            let dest = t.strip();
                            let name = c.st.name(dest);
                            ui.label(egui::RichText::new(name).color(theme::strip_color(c.st.color(dest))));
                            let v = c.st.send_level(s, t);
                            let id = egui::Id::unique(("send", t));
                            if let Some(nv) = widgets::hbar(ui, Vec2::new(150.0, 12.0), v, false, id) {
                                c.actions.push(Action::Set(Param::SendLevel(s, t), Value::Float(fader::quantize(nv))));
                            }
                            ui.label(egui::RichText::new(widgets::fmt_db(fader::to_db(v))).monospace().size(11.0));
                            ui.end_row();
                        }
                    });
                    ui.add_space(16.0);
                }
            });
        });
    });
}

fn section(ui: &mut Ui, title: &str) {
    ui.label(egui::RichText::new(title).size(11.0).strong().color(theme::DIM));
}
