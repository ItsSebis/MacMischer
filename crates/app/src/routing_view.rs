//! Routing window: channel input sources and output sources / tap points.

use crate::actions::Action;
use crate::theme;
use eframe::egui::{self, RichText};
use xair_core::MixerState;
use xair_proto::routing::{self, Output};
use xair_proto::{Param, StripId, Value};

fn geti(st: &MixerState, p: Param) -> Option<i32> {
    st.get(p).and_then(Value::as_i32)
}

fn set(actions: &mut Vec<Action>, p: Param, v: i32) {
    actions.push(Action::Set(p, Value::Int(v)));
}

/// Dropdown over `0..count` with labels from `label`; emits a Set when changed.
fn combo(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    cur: Option<i32>,
    count: i32,
    label: impl Fn(i32) -> String,
    param: Param,
    actions: &mut Vec<Action>,
) {
    let shown = cur.map_or("…".to_string(), &label);
    egui::ComboBox::from_id_salt(salt).selected_text(shown).width(110.0).show_ui(ui, |ui| {
        for code in 0..count {
            if ui.selectable_label(cur == Some(code), label(code)).clicked() && cur != Some(code) {
                set(actions, param, code);
            }
        }
    });
}

fn input_row(ui: &mut egui::Ui, st: &MixerState, s: StripId, actions: &mut Vec<Action>) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(6.0, 16.0), egui::Sense::hover());
    ui.painter().rect_filled(r, 2.0, theme::strip_color(st.color(s)));
    ui.label(st.name(s));
    let usb = geti(st, Param::RtnSw(s)).unwrap_or(0) != 0;
    ui.horizontal(|ui| {
        if ui.selectable_label(!usb, "Analog").clicked() && usb {
            set(actions, Param::RtnSw(s), 0);
        }
        if ui.selectable_label(usb, "USB").clicked() && !usb {
            set(actions, Param::RtnSw(s), 1);
        }
    });
    let salt = ("in-src", s);
    match (s, usb) {
        (StripId::AuxIn, _) => {
            combo(ui, salt, geti(st, Param::RtnSrc(s)), 9, routing::aux_usb_pair_label, Param::RtnSrc(s), actions)
        }
        (_, true) => {
            combo(ui, salt, geti(st, Param::RtnSrc(s)), 18, routing::usb_return_label, Param::RtnSrc(s), actions)
        }
        (_, false) => {
            combo(ui, salt, geti(st, Param::InSrc(s)), 16, routing::analog_in_label, Param::InSrc(s), actions)
        }
    }
    ui.end_row();
}

fn output_grid(ui: &mut egui::Ui, st: &MixerState, outs: Vec<Output>, actions: &mut Vec<Action>) {
    egui::Grid::new(("out-grid", outs[0].prefix())).num_columns(3).spacing([12.0, 4.0]).show(ui, |ui| {
        for o in outs {
            ui.label(o.label());
            combo(ui, ("out-src", o), geti(st, Param::RouteSrc(o)), routing::SOURCE_COUNT, routing::source_label, Param::RouteSrc(o), actions);
            combo(ui, ("out-pos", o), geti(st, Param::RoutePos(o)), routing::TAP_COUNT, |c| routing::tap_label(c).to_string(), Param::RoutePos(o), actions);
            ui.end_row();
        }
    });
}

/// Routing window. Emits `Action::Set(param, Value::Int(..))` for edits. `locked` = read-only (safety mode).
pub fn routing_window(
    ctx: &egui::Context,
    open: &mut bool,
    st: &MixerState,
    locked: bool,
    actions: &mut Vec<Action>,
) {
    let max_h = ctx.content_rect().height() * 0.9;
    egui::Window::new("Routing")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(560.0)
        .max_height(max_h)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            if locked {
                ui.colored_label(theme::WARN, "Console is locked — routing is read-only. Ctrl/Cmd+L or the Lock button unlocks.");
            }
            egui::ScrollArea::vertical().max_height(max_h - 60.0).min_scrolled_height(max_h - 60.0).auto_shrink([false, false]).show(ui, |ui| {
                ui.add_enabled_ui(!locked, |ui| {
                    egui::CollapsingHeader::new("Inputs").default_open(true).show(ui, |ui| {
                        egui::Grid::new("in-grid").num_columns(4).spacing([12.0, 4.0]).show(ui, |ui| {
                            for c in 1..=xair_proto::strip::CHANNELS {
                                input_row(ui, st, StripId::Ch(c), actions);
                            }
                            input_row(ui, st, StripId::AuxIn, actions);
                        });
                    });
                    egui::CollapsingHeader::new("USB sends (to computer)").default_open(true).show(ui, |ui| {
                        output_grid(ui, st, Output::Usb(1).all_of_kind(), actions);
                    });
                    egui::CollapsingHeader::new("Aux outputs").show(ui, |ui| {
                        output_grid(ui, st, Output::Aux(1).all_of_kind(), actions);
                    });
                    egui::CollapsingHeader::new("Ultranet (P16)").show(ui, |ui| {
                        output_grid(ui, st, Output::P16(1).all_of_kind(), actions);
                    });
                });
                egui::CollapsingHeader::new("Main outputs").show(ui, |ui| {
                    egui::Grid::new("main-grid").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                        for o in Output::Main(1).all_of_kind() {
                            ui.label(o.label());
                            let t = match geti(st, Param::RouteSrc(o)) {
                                Some(0) if o == Output::Main(1) => "LR".to_string(),
                                Some(1) if o == Output::Main(2) => "MON".to_string(),
                                Some(c) => routing::source_label(c),
                                None => "…".into(),
                            };
                            ui.label(RichText::new(t).color(theme::DIM));
                            ui.end_row();
                        }
                    });
                });
            });
        });
}
