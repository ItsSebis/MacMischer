//! FX rack window: the four effect slots with type selector and raw parameter bars.
//!
//! Effect type names and per-type parameter meanings/scales are NOT hardware-verified yet
//! (the mixer was unreachable when this was written), so types show as codes and parameters
//! as normalised 0..100 % values. Edits write exact 0..1 floats.

use crate::actions::Action;
use crate::theme;
use crate::widgets;
use eframe::egui::{self, RichText, Vec2};
use xair_core::MixerState;
use xair_proto::param::FX_PARAMS;
use xair_proto::strip::FX;
use xair_proto::{Param, StripId, Value};

/// Number of selectable effect type codes offered in the dropdown.
const TYPE_CODES: i32 = 64;

fn slot_ui(ui: &mut egui::Ui, st: &MixerState, slot: u8, actions: &mut Vec<Action>) {
    let ty = st.fx_type(slot);
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("FX {slot}")).strong());
        ui.label(RichText::new(format!("send: {}", st.name(StripId::FxSend(slot)))).size(11.0).color(theme::DIM));
        ui.label(RichText::new(format!("return: {}", st.name(StripId::FxRtn(slot)))).size(11.0).color(theme::DIM));
        let mut sel = ty;
        egui::ComboBox::from_id_salt(("fx-type", slot))
            .width(90.0)
            .selected_text(format!("Type {ty}"))
            .show_ui(ui, |ui| {
                for t in 0..TYPE_CODES {
                    ui.selectable_value(&mut sel, t, format!("Type {t}"));
                }
            });
        if sel != ty {
            actions.push(Action::Set(Param::FxType(slot), Value::Int(sel)));
        }
    });
    egui::CollapsingHeader::new("Parameters").id_salt(("fx-pars", slot)).show(ui, |ui| {
        egui::Grid::new(("fx-grid", slot)).num_columns(6).spacing([8.0, 3.0]).show(ui, |ui| {
            for i in 1..=FX_PARAMS {
                let x = st.fx_par(slot, i);
                ui.label(RichText::new(format!("P{i}")).size(11.0).color(theme::DIM));
                if let Some(n) = widgets::hbar(ui, Vec2::new(90.0, 14.0), x, false, egui::Id::unique(("fx-par", slot, i))) {
                    actions.push(Action::Set(Param::FxPar(slot, i), Value::Float(n.clamp(0.0, 1.0))));
                }
                ui.label(RichText::new(format!("{:.0} %", x * 100.0)).size(11.0));
                if i % 2 == 0 {
                    ui.end_row();
                }
            }
        });
    });
}

/// FX rack window. `locked` = read-only (safety mode).
pub fn fx_window(ctx: &egui::Context, open: &mut bool, st: &MixerState, locked: bool, actions: &mut Vec<Action>) {
    let max_h = ctx.content_rect().height() * 0.9;
    egui::Window::new("Effects")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .max_height(max_h)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            if locked {
                ui.colored_label(theme::WARN, "Console is locked — effects are read-only.");
            }
            ui.label(
                RichText::new("Type names and parameter units are not verified on hardware yet; values are raw 0..100 %.")
                    .size(11.0)
                    .color(theme::DIM),
            );
            egui::ScrollArea::vertical().max_height(max_h - 90.0).auto_shrink([true, true]).show(ui, |ui| {
                ui.add_enabled_ui(!locked, |ui| {
                    for slot in 1..=FX {
                        slot_ui(ui, st, slot, actions);
                        ui.separator();
                    }
                });
            });
        });
}
