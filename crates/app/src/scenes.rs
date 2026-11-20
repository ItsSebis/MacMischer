//! Scenes (mixer snapshots 1–64): recall, save, rename — with confirmation where it matters.

use crate::actions::Action;
use crate::theme;
use eframe::egui;
use std::time::{Duration, Instant};
use xair_core::MixerState;

pub const SLOTS: u8 = 64;
const CONFIRM_FOR: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy, PartialEq)]
enum Pending {
    Recall(u8),
    Save(u8),
}

#[derive(Default)]
pub struct ScenesUi {
    pub open: bool,
    pending: Option<(Pending, Instant)>,
    /// Slot whose name is being edited, and the edit buffer.
    editing: Option<(u8, String)>,
    filter_empty: bool,
}

impl ScenesUi {
    fn armed(&self, p: Pending) -> bool {
        matches!(self.pending, Some((x, t)) if x == p && t.elapsed() < CONFIRM_FOR)
    }

    /// Two-step confirm: first call arms, second (within 4 s) returns true.
    fn confirm(&mut self, p: Pending) -> bool {
        if self.armed(p) {
            self.pending = None;
            true
        } else {
            self.pending = Some((p, Instant::now()));
            false
        }
    }

    /// `safe` = Safe mode (recall needs confirmation).
    pub fn window(&mut self, ctx: &egui::Context, st: &MixerState, safe: bool, actions: &mut Vec<Action>) {
        if !self.open {
            return;
        }
        let mut open = self.open;
        let current = st.snap_index();
        let max_h = ctx.content_rect().height() * 0.85;
        egui::Window::new("Scenes")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .max_height(max_h)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("Current: {current}  {}", st.snap_name(current.max(1) as u8))).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.checkbox(&mut self.filter_empty, "Hide empty");
                    });
                });
                if safe {
                    ui.label(egui::RichText::new("Safe mode: click Recall twice to confirm.").size(11.0).color(theme::DIM));
                }
                ui.separator();
                egui::ScrollArea::vertical().max_height(max_h - 120.0).show(ui, |ui| {
                    egui::Grid::new("scenes").num_columns(4).spacing([10.0, 6.0]).striped(true).show(ui, |ui| {
                        for i in 1..=SLOTS {
                            let name = st.snap_name(i);
                            if self.filter_empty && name.is_empty() && i as i32 != current {
                                continue;
                            }
                            self.row(ui, i, &name, i as i32 == current, safe, actions);
                            ui.end_row();
                        }
                    });
                });
            });
        self.open = open;
        if self.pending.is_some_and(|(_, t)| t.elapsed() >= CONFIRM_FOR) {
            self.pending = None;
        }
        if self.pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    fn row(&mut self, ui: &mut egui::Ui, i: u8, name: &str, current: bool, safe: bool, actions: &mut Vec<Action>) {
        let num = egui::RichText::new(format!("{i:>2}")).monospace();
        ui.label(if current { num.color(theme::ACCENT).strong() } else { num.color(theme::DIM) });

        // Name (click to rename)
        match &mut self.editing {
            Some((slot, buf)) if *slot == i => {
                let r = ui.add(egui::TextEdit::singleline(buf).desired_width(220.0).char_limit(16));
                r.request_focus();
                if r.lost_focus() {
                    if !ui.input(|inp| inp.key_pressed(egui::Key::Escape)) {
                        actions.push(Action::RenameScene(i, buf.trim().to_string()));
                    }
                    self.editing = None;
                }
            }
            _ => {
                let text = if name.is_empty() {
                    egui::RichText::new("(empty)").italics().color(theme::DIM)
                } else if current {
                    egui::RichText::new(name).strong().color(theme::ACCENT)
                } else {
                    egui::RichText::new(name)
                };
                let r = ui.add_sized([220.0, 20.0], egui::Label::new(text).sense(egui::Sense::click()));
                if r.on_hover_text("Click to rename").clicked() {
                    self.editing = Some((i, name.to_string()));
                }
            }
        }

        // Recall
        let armed = self.armed(Pending::Recall(i));
        let label = if armed { "Confirm recall" } else { "Recall" };
        let mut b = egui::Button::new(label).min_size(egui::vec2(110.0, 0.0));
        if armed {
            b = b.fill(theme::WARN.linear_multiply(0.5));
        }
        if ui.add_enabled(!name.is_empty() || current, b).clicked() && (!safe || self.confirm(Pending::Recall(i))) {
            actions.push(Action::RecallScene(i));
        }

        // Save (overwriting a named scene always asks)
        let armed = self.armed(Pending::Save(i));
        let label = if armed { "Overwrite?" } else { "Save here" };
        let mut b = egui::Button::new(label).min_size(egui::vec2(90.0, 0.0));
        if armed {
            b = b.fill(theme::MUTE.linear_multiply(0.5));
        }
        if ui.add(b).clicked() && (name.is_empty() || self.confirm(Pending::Save(i))) {
            let new_name = if name.is_empty() { format!("Scene {i}") } else { name.to_string() };
            actions.push(Action::SaveScene(i, new_name));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_step_confirm() {
        let mut s = ScenesUi::default();
        assert!(!s.confirm(Pending::Recall(3)));
        assert!(!s.confirm(Pending::Recall(4))); // different target re-arms
        assert!(s.confirm(Pending::Recall(4)));
        assert!(!s.confirm(Pending::Recall(4))); // consumed
    }
}
