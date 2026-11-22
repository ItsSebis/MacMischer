//! MIDI settings window: enable, device, binding table, Learn.

use crate::midi::input::LearnState;
use crate::midi::{LearnReq, MidiPrefs, MidiService, MidiStatus, Target, TargetKind};
use crate::theme;
use crate::widgets::{icon_button, Icon};
use eframe::egui;
use std::collections::HashMap;
use xair_proto::StripId;

pub struct MidiUi {
    pub open: bool,
    pub learn: LearnState,
    kind: TargetKind,
    strip: StripId,
    scene: u8,
}

impl Default for MidiUi {
    fn default() -> Self {
        MidiUi { open: false, learn: LearnState::default(), kind: TargetKind::Fader, strip: StripId::Ch(1), scene: 1 }
    }
}

fn target_label(t: Target, names: &HashMap<StripId, String>) -> String {
    let nm = |s: StripId| match names.get(&s).filter(|n| !n.is_empty()) {
        Some(n) => format!("{} {n}", s.short()),
        None => s.short(),
    };
    match t {
        Target::StripFader(s) => format!("Fader  {}", nm(s)),
        Target::StripPan(s) => format!("Pan  {}", nm(s)),
        Target::StripMute(s) => format!("Mute  {}", nm(s)),
        Target::StripSolo(s) => format!("Solo  {}", nm(s)),
        Target::Select(s) => format!("Select  {}", nm(s)),
        Target::RecallScene(n) => format!("Recall scene {n}"),
    }
}

/// Returns true when the preferences changed (caller saves them).
pub fn midi_window(
    ctx: &egui::Context,
    ui_state: &mut MidiUi,
    cfg: &mut MidiPrefs,
    svc: &MidiService,
    names: &HashMap<StripId, String>,
    safe_mode: bool,
) -> bool {
    if !ui_state.open {
        ui_state.learn.learning = None;
        return false;
    }
    let mut changed = false;
    let mut open = ui_state.open;
    let max_h = ctx.content_rect().height() * 0.85;
    let ports = svc.ports();
    egui::Window::new("MIDI")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .max_height(max_h)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(max_h - 50.0).auto_shrink([true, true]).show(ui, |ui| {
                ui.set_width(560.0);
                if !MidiService::available() {
                    ui.colored_label(theme::WARN, "This build has no MIDI support (cargo feature `midi`).");
                }
                changed |= ui.checkbox(&mut cfg.enabled, "Enable MIDI input").changed();
                ui.horizontal(|ui| {
                    ui.label("Device");
                    let cur = cfg.device.clone().unwrap_or_else(|| "(none)".into());
                    egui::ComboBox::from_id_salt("midi-dev").selected_text(cur).width(300.0).show_ui(ui, |ui| {
                        changed |= ui.selectable_value(&mut cfg.device, None, "(none)").changed();
                        for p in &ports {
                            changed |= ui.selectable_value(&mut cfg.device, Some(p.clone()), p).changed();
                        }
                        // A remembered device that is currently unplugged stays selectable.
                        if let Some(d) = cfg.device.clone().filter(|d| !ports.contains(d)) {
                            ui.label(egui::RichText::new(format!("{d} (not connected)")).color(theme::DIM));
                        }
                    });
                });
                let (text, color) = match svc.status() {
                    MidiStatus::Off => ("Off".to_string(), theme::DIM),
                    MidiStatus::NoDeviceChosen => ("Choose a device".to_string(), theme::WARN),
                    MidiStatus::Waiting(d) => (format!("Waiting for {d} (checking every 2 s)"), theme::WARN),
                    MidiStatus::Connected(d) => (format!("Connected: {d}"), theme::OK),
                    MidiStatus::Error(e) => (e, theme::MUTE),
                };
                ui.label(egui::RichText::new(text).color(color));
                if let Some(m) = ui_state.learn.last_msg {
                    ui.label(egui::RichText::new(format!("Last message: {}", m.describe())).monospace().size(11.0).color(theme::DIM));
                }
                if safe_mode {
                    ui.label(
                        egui::RichText::new("Safe mode is on: MIDI scene recall is refused (it cannot be confirmed). Switch to Simple to allow it.")
                            .size(11.0)
                            .color(theme::WARN),
                    );
                }
                ui.label(
                    egui::RichText::new(
                        "Faders and pans use soft takeover: move the controller past the current position first. \
                         Fader CC 95 is about unity; values are capped (default +6 dB). The console lock applies to MIDI too.",
                    )
                    .size(11.0)
                    .color(theme::DIM),
                );
                ui.separator();
                bindings_table(ui, ui_state, cfg, names, &mut changed);
                ui.separator();
                add_form(ui, ui_state, names);
                if let Some(n) = &ui_state.learn.note {
                    ui.colored_label(theme::WARN, n);
                }
            });
        });
    ui_state.open = open;
    if !open {
        ui_state.learn.learning = None;
    }
    changed
}

fn bindings_table(ui: &mut egui::Ui, st: &mut MidiUi, cfg: &mut MidiPrefs, names: &HashMap<StripId, String>, changed: &mut bool) {
    ui.label(egui::RichText::new("Bindings").strong());
    if cfg.bindings.is_empty() {
        ui.label(egui::RichText::new("None yet - pick a target below and press Learn.").color(theme::DIM));
        return;
    }
    let mut remove = None;
    egui::Grid::new("midi-binds").num_columns(4).spacing([12.0, 6.0]).striped(true).show(ui, |ui| {
        for (i, b) in cfg.bindings.iter_mut().enumerate() {
            ui.label(egui::RichText::new(b.source_label()).monospace());
            ui.label(target_label(b.target, names));
            ui.horizontal(|ui| {
                if let (Target::StripFader(_), Some(db)) = (b.target, &mut b.max_db) {
                    let r = ui.add(egui::DragValue::new(db).range(-60.0..=10.0).speed(0.25).suffix(" dB max").fixed_decimals(1));
                    if r.changed() {
                        *db = (*db * 2.0).round() / 2.0;
                    }
                    // Save once per edit, not on every frame of a drag.
                    if r.drag_stopped() || (r.changed() && !r.dragged()) {
                        *changed = true;
                    }
                }
            });
            ui.horizontal(|ui| {
                let learning = st.learn.learning.is_some_and(|l| l.replace == Some(i));
                let label = if learning { "Move a control..." } else { "Relearn" };
                if ui.selectable_label(learning, label).clicked() {
                    st.learn.learning = (!learning).then_some(LearnReq { target: b.target, replace: Some(i) });
                    st.learn.note = None;
                }
                if icon_button(ui, Icon::Remove, true).on_hover_text("Remove binding").clicked() {
                    remove = Some(i);
                }
            });
            ui.end_row();
        }
    });
    if let Some(i) = remove {
        cfg.bindings.remove(i);
        st.learn.learning = None;
        *changed = true;
    }
}

fn add_form(ui: &mut egui::Ui, st: &mut MidiUi, names: &HashMap<StripId, String>) {
    ui.label(egui::RichText::new("Add binding").strong());
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("midi-kind").selected_text(st.kind.label()).show_ui(ui, |ui| {
            for k in TargetKind::ALL {
                ui.selectable_value(&mut st.kind, k, k.label());
            }
        });
        if st.kind == TargetKind::Scene {
            ui.add(egui::DragValue::new(&mut st.scene).range(1..=crate::scenes::SLOTS));
        } else {
            let label = |s: StripId| match names.get(&s).filter(|n| !n.is_empty()) {
                Some(n) => format!("{}  {n}", s.short()),
                None => s.short(),
            };
            egui::ComboBox::from_id_salt("midi-strip").selected_text(label(st.strip)).height(300.0).show_ui(ui, |ui| {
                for s in StripId::all() {
                    if st.kind == TargetKind::Pan && !s.has_pan() {
                        continue;
                    }
                    ui.selectable_value(&mut st.strip, s, label(s));
                }
            });
        }
        let target = st.kind.build(st.strip, st.scene);
        let learning = st.learn.learning.is_some_and(|l| l.replace.is_none());
        let text = if learning { "Move a control... (click to cancel)" } else { "Learn" };
        if ui.selectable_label(learning, text).clicked() {
            if learning || target.validate().is_err() {
                st.learn.learning = None;
            } else {
                st.learn.learning = Some(LearnReq { target, replace: None });
                st.learn.note = None;
            }
        }
    });
    if st.kind == TargetKind::Pan && !st.strip.has_pan() {
        st.strip = StripId::Ch(1);
    }
}
