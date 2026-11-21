//! Console views: channel strip, meter bridge, detail panel.

use crate::actions::Action;
use crate::theme;
use crate::widgets::{self, Ballistics};
use eframe::egui::{self, Align2, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::collections::HashMap;
use xair_core::{Meters, MixerState};
use xair_proto::{fader, Param, SendTo, StripId, Value};

pub const STRIP_W: f32 = 86.0;
pub const STRIP_W_STEREO: f32 = 118.0;

/// Everything a view needs for one frame (read-only) plus the action sink.
pub struct Ctx<'a> {
    pub st: &'a MixerState,
    pub meters: &'a Meters,
    pub ball: &'a mut HashMap<StripId, Ballistics>,
    pub selected: StripId,
    pub actions: &'a mut Vec<Action>,
    pub view: DetailView,
    /// A/B compare slot loaded for the selected strip, if started.
    pub ab: Option<crate::ab::Slot>,
    /// Saved preset names (cached; refreshed after save/delete).
    pub presets: &'a [String],
    /// An "Undo preset" is available for the selected strip.
    pub preset_undo: bool,
}

/// What the detail panel shows for the selected strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailView {
    #[default]
    Mix,
    Eq,
    Dynamics,
}

/// A/B compare buttons (also bound to `b`); the loaded slot is highlighted.
pub fn ab_buttons(ui: &mut Ui, c: &mut Ctx) {
    use crate::ab::Slot;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("A/B").size(10.0).color(crate::theme::DIM));
        for (slot, name) in [(Slot::A, "A"), (Slot::B, "B")] {
            let on = c.ab == Some(slot);
            let r = ui
                .add(egui::Button::new(name).selected(on).min_size(Vec2::new(24.0, 0.0)))
                .on_hover_text("Compare two versions of this strip's EQ / dynamics (key: b)");
            if r.clicked() {
                c.actions.push(Action::AbSelect(slot));
            }
        }
        ui.separator();
        preset_menu(ui, c);
    });
}

/// "Presets" menu: load / save as / delete named processing presets for the selected strip.
pub fn preset_menu(ui: &mut Ui, c: &mut Ctx) {
    #[derive(Clone, PartialEq)]
    enum Pending {
        Delete(String),
        Overwrite(String),
    }
    #[derive(Clone, Default)]
    struct St {
        name: String,
        was_open: bool,
        /// Awaiting confirmation, for this strip only.
        pending: Option<(Pending, StripId)>,
    }
    let id = ui.make_persistent_id("preset_menu");
    let mut st: St = ui.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    let has_proc = !xair_proto::param::proc_params(c.selected).is_empty();
    if st.pending.as_ref().is_some_and(|(_, s)| *s != c.selected) {
        st.pending = None;
    }
    let mut close = false;
    let r = ui.menu_button("Presets", |ui| {
        ui.set_min_width(220.0);
        if !st.was_open {
            c.actions.push(Action::PresetRefresh);
        }
        if c.presets.is_empty() {
            ui.label(egui::RichText::new("No presets saved yet").color(crate::theme::DIM));
        }
        for n in c.presets {
            if matches!(&st.pending, Some((Pending::Delete(p), _)) if p == n) {
                ui.horizontal(|ui| {
                    ui.label(format!("Delete \"{n}\"?"));
                    if ui.button("Yes").clicked() {
                        c.actions.push(Action::PresetDelete(n.clone()));
                        st.pending = None;
                    }
                    if ui.button("No").clicked() {
                        st.pending = None;
                    }
                });
                continue;
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(has_proc, egui::Button::new("Load")).on_hover_text("Apply to the selected strip").clicked() {
                    c.actions.push(Action::PresetLoad(n.clone()));
                    close = true;
                }
                if ui.button("Delete").clicked() {
                    st.pending = Some((Pending::Delete(n.clone()), c.selected));
                }
                ui.label(n);
            });
        }
        ui.separator();
        let strip = format!("{} {}", c.selected.short(), c.st.name(c.selected));
        if let Some((Pending::Overwrite(n), _)) = st.pending.clone() {
            ui.horizontal(|ui| {
                ui.label(format!("Overwrite \"{n}\" with {}?", strip.trim()));
                if ui.button("Yes").clicked() {
                    c.actions.push(Action::PresetSave(n));
                    st.pending = None;
                    st.name.clear();
                    close = true;
                }
                if ui.button("No").clicked() {
                    st.pending = None;
                }
            });
        } else {
            ui.horizontal(|ui| {
                ui.label("Save as");
                let r = ui.add(egui::TextEdit::singleline(&mut st.name).desired_width(120.0).char_limit(crate::presets::MAX_NAME));
                let ok = has_proc && !st.name.trim().is_empty();
                let go = ui.add_enabled(ok, egui::Button::new("Save")).clicked()
                    || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && ok);
                if go {
                    match crate::presets::find_existing(c.presets, &st.name) {
                        Some(existing) => st.pending = Some((Pending::Overwrite(existing.clone()), c.selected)),
                        None => {
                            c.actions.push(Action::PresetSave(st.name.trim().to_string()));
                            st.name.clear();
                            close = true;
                        }
                    }
                }
            });
        }
        if c.preset_undo && ui.button("Undo preset").clicked() {
            c.actions.push(Action::PresetUndo);
            close = true;
        }
        if close {
            ui.close();
        }
    });
    st.was_open = r.inner.is_some();
    if !st.was_open || close {
        st.pending = None;
    }
    ui.data_mut(|d| d.insert_temp(id, st));
}

impl Ctx<'_> {
    fn meter(&mut self, s: StripId) -> (f32, f32) {
        let raw = self.meters.strip(s).unwrap_or(-128.0);
        self.ball.entry(s).or_default().update(raw)
    }
}

/// Name tag with strip colour; click selects.
fn name_tag(ui: &mut Ui, c: &mut Ctx, s: StripId, width: f32) {
    name_tag_with(ui, c, s, width, c.st.name(s), 10);
}

fn name_tag_with(ui: &mut Ui, c: &mut Ctx, s: StripId, width: f32, mut name: String, max_chars: usize) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::click());
    let col = theme::strip_color(c.st.color(s));
    let inverted = c.st.color(s) >= 8;
    let p = ui.painter();
    let (bg, fg) = if inverted { (col, egui::Color32::BLACK) } else { (theme::BG, col) };
    p.rect_filled(rect, 3.0, bg);
    p.rect_stroke(rect, 3.0, Stroke::new(1.5, col), StrokeKind::Inside);
    if name.chars().count() > max_chars {
        name = name.chars().take(max_chars).collect();
    }
    p.text(rect.center(), Align2::CENTER_CENTER, name, FontId::proportional(12.5), fg);
    if resp.clicked() {
        c.actions.push(Action::Select(s));
    }
}

/// One channel strip. `send` = bank is bound to a bus: fader controls that send.
/// `pos` = 1-based bank position shown in the footer (positional banks).
pub fn strip(ui: &mut Ui, c: &mut Ctx, s: StripId, height: f32, send: Option<SendTo>, pos: Option<usize>) {
    // A stereo-linked pair renders as one wide strip (leader = odd strip).
    let partner = c.st.linked_partner(s).filter(|p| *p > s);
    let sel = c.selected == s || Some(c.selected) == partner;
    let send = send.filter(|_| s.has_sends());
    let width = if partner.is_some() { STRIP_W_STEREO } else { STRIP_W };
    let (frame_rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
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

    match partner {
        Some(p) => {
            let (a, b) = (c.st.name(s), c.st.name(p));
            let both = format!("{a} · {b}");
            let label = if both.chars().count() <= 15 { both } else { a };
            name_tag_with(ui, c, s, w, label, 15);
        }
        None => name_tag(ui, c, s, w),
    }
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
        let mw = if partner.is_some() { 21.0 } else { 10.0 };
        let (mrect, mresp) = ui.allocate_exact_size(Vec2::new(mw, fh), Sense::click());
        let (lvl, peak) = c.meter(s);
        if let Some(p) = partner {
            let (rl, rp) = c.meter(p);
            let half = Rect::from_min_size(mrect.min, Vec2::new(10.0, fh));
            widgets::paint_meter(ui, half, lvl, peak);
            widgets::paint_meter(ui, half.translate(Vec2::new(11.0, 0.0)), rl, rp);
        } else if !matches!(s, StripId::Dca(_)) {
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
        if let Some(v) = widgets::fader(ui, Vec2::new(w - mw - 4.0, fh), value, accent, id) {
            c.actions.push(Action::Set(param, Value::Float(v)));
            c.actions.push(Action::Select(s));
        }
    });

    if partner.is_some() {
        ui.label(egui::RichText::new("STEREO").size(9.5).color(theme::DIM));
    } else if s.has_pan() && send.is_none() {
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
        let soloed = c.st.soloed(s);
        if widgets::toggle(ui, Vec2::new(bw, 22.0), "S", soloed, theme::SOLO).clicked() {
            c.actions.push(Action::SetSolo(s, !soloed));
        }
    });
    ui.horizontal(|ui| {
        if let Some(p) = pos {
            ui.label(egui::RichText::new(format!("#{p}")).size(12.0).strong().color(theme::ACCENT));
        }
        let label = match partner {
            Some(p) => format!("{}-{}", s.short(), p.short().trim_start_matches('B')),
            None => s.short(),
        };
        ui.label(egui::RichText::new(label).size(11.0).color(theme::DIM));
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
                let soloed = c.st.soloed(s);
                if widgets::toggle(ui, Vec2::new(64.0, 28.0), "SOLO", soloed, theme::SOLO).clicked() {
                    c.actions.push(Action::SetSolo(s, !soloed));
                }
            });
            ui.add_space(6.0);
            // View tabs (keys: v / e / c)
            ui.horizontal(|ui| {
                let has_eq = xair_proto::proc::eq_bands(s) > 0;
                let has_dyn = xair_proto::proc::has_dyn(s) || xair_proto::proc::has_gate(s);
                for (v, label, enabled) in [
                    (DetailView::Mix, "Mix", true),
                    (DetailView::Eq, "EQ", has_eq),
                    (DetailView::Dynamics, "Dyn", has_dyn),
                ] {
                    let r = ui.add_enabled(enabled, egui::Button::new(label).selected(c.view == v).min_size(Vec2::new(58.0, 22.0)));
                    if r.clicked() {
                        c.actions.push(Action::SetView(v));
                    }
                }
            });
        });
        ui.separator();

        match c.view {
            DetailView::Eq if xair_proto::proc::eq_bands(s) > 0 => {
                crate::eq_view::eq_view(ui, c, s);
                return;
            }
            DetailView::Dynamics if xair_proto::proc::has_dyn(s) || xair_proto::proc::has_gate(s) => {
                crate::dyn_view::dyn_view(ui, c, s);
                return;
            }
            _ => {}
        }

        if let StripId::Dca(n) = s {
            dca_members(ui, c, n);
            return;
        }

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
            if s.has_groups() {
                ui.add_space(6.0);
                group_row(ui, c, Param::DcaGroups(s), "DCA", "D");
                group_row(ui, c, Param::MuteGroups(s), "Mute groups", "M");
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

/// Label + 4 toggles editing a group bitmask param (`DcaGroups` / `MuteGroups`).
fn group_row(ui: &mut Ui, c: &mut Ctx, p: Param, title: &str, short: &str) {
    let mask = c.st.get(p).and_then(Value::as_i32).unwrap_or(0);
    ui.horizontal(|ui| {
        ui.add_sized(Vec2::new(78.0, 22.0), egui::Label::new(egui::RichText::new(title).size(11.0).color(theme::DIM)));
        for g in 1..=4u8 {
            let bit = 1 << (g - 1);
            let on = mask & bit != 0;
            let (col, hover) = match p {
                Param::DcaGroups(_) => {
                    let d = StripId::Dca(g);
                    (theme::strip_color(c.st.color(d)), c.st.name(d))
                }
                _ => (theme::MUTE, format!("Mute group {g}")),
            };
            if widgets::toggle(ui, Vec2::new(30.0, 22.0), &format!("{short}{g}"), on, col).on_hover_text(hover).clicked() {
                c.actions.push(Action::Set(p, Value::Int(mask ^ bit)));
            }
        }
    });
}

/// Mute group masters + monitor level (right column above Main).
pub fn master_section(ui: &mut Ui, c: &mut Ctx) {
    ui.label(egui::RichText::new("MUTE GROUPS").size(10.0).color(theme::DIM));
    for row in [[1u8, 2], [3, 4]] {
        ui.horizontal(|ui| {
            for g in row {
                let on = c.st.mute_group_on(g);
                if widgets::toggle(ui, Vec2::new(38.0, 26.0), &format!("M{g}"), on, theme::MUTE).clicked() {
                    c.actions.push(Action::Set(Param::MuteGroupOn(g), Value::Int(!on as i32)));
                }
            }
        });
    }
    ui.add_space(4.0);
    let v = c.st.monitor_level();
    ui.label(egui::RichText::new(format!("PHONES {}", widgets::fmt_db(fader::to_db(v)))).size(10.0).color(theme::DIM));
    if let Some(nv) = widgets::hbar(ui, Vec2::new(80.0, 12.0), v, false, egui::Id::unique("monitor")) {
        c.actions.push(Action::Set(Param::MonitorLevel, Value::Float(fader::quantize(nv))));
    }
    ui.add_space(6.0);
}

/// DCA detail: toggle which strips belong to DCA `n`.
fn dca_members(ui: &mut Ui, c: &mut Ctx, n: u8) {
    let bit = 1 << (n - 1);
    ui.vertical(|ui| {
        section(ui, "MEMBERS  (click to assign / remove)");
        let inputs: Vec<StripId> = StripId::all().into_iter().filter(|s| s.has_groups()).collect();
        for row in inputs.chunks(11) {
            ui.horizontal(|ui| {
                for &s in row {
                    let mask = c.st.dca_mask(s);
                    let on = mask & bit != 0;
                    let col = theme::strip_color(c.st.color(s));
                    let mut name = c.st.name(s);
                    if matches!(s, StripId::Bus(_) | StripId::FxSend(_)) {
                        name = format!("{} {name}", s.short());
                    }
                    if name.chars().count() > 9 {
                        name = name.chars().take(9).collect();
                    }
                    let r = widgets::toggle(ui, Vec2::new(78.0, 30.0), &name, on, col).on_hover_text(s.to_string());
                    if r.clicked() {
                        c.actions.push(Action::Set(Param::DcaGroups(s), Value::Int(mask ^ bit)));
                    }
                }
            });
        }
    });
}

fn section(ui: &mut Ui, title: &str) {
    ui.label(egui::RichText::new(title).size(11.0).strong().color(theme::DIM));
}
