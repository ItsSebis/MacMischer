//! Modal editor for one fader bank: name, strips (add / reorder / remove), bus binding.

use crate::banks::Bank;
use crate::theme;
use eframe::egui;
use std::collections::HashMap;
use xair_proto::{SendTo, StripId};

pub struct BankEditor {
    index: usize,
    draft: Bank,
}

impl BankEditor {
    pub fn open(index: usize, bank: &Bank) -> BankEditor {
        BankEditor { index, draft: bank.clone() }
    }

    /// Returns `Some(Some(..))` on save, `Some(None)` on cancel, `None` while open.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        names: &HashMap<StripId, String>,
    ) -> Option<Option<(usize, Bank)>> {
        let mut result = None;
        let label = |s: StripId| format!("{:<4} {}", s.short(), names.get(&s).cloned().unwrap_or_default());
        egui::Window::new("Edit bank")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut self.draft.name);
                });
                ui.horizontal(|ui| {
                    ui.label("Faders control");
                    let cur = match self.draft.bus {
                        None => "Channel levels".to_string(),
                        Some(t) => format!("Sends to {}", names.get(&t.strip()).cloned().unwrap_or_default()),
                    };
                    egui::ComboBox::from_id_salt("bus").selected_text(cur).show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.draft.bus, None, "Channel levels");
                        for t in SendTo::all() {
                            let n = format!("Sends to {} ({})", names.get(&t.strip()).cloned().unwrap_or_default(), t.strip());
                            ui.selectable_value(&mut self.draft.bus, Some(t), n);
                        }
                    });
                });
                if self.draft.bus.is_some() {
                    ui.label(
                        egui::RichText::new("Monitor bank: input faders set their send to this bus.")
                            .color(theme::DIM)
                            .size(11.0),
                    );
                }
                ui.separator();
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(260.0);
                        ui.label(egui::RichText::new("In this bank").strong());
                        egui::ScrollArea::vertical().id_salt("in").max_height(360.0).show(ui, |ui| {
                            let mut op = None;
                            let n = self.draft.strips.len();
                            for (i, s) in self.draft.strips.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    if ui.add_enabled(i > 0, egui::Button::new("↑").small()).clicked() {
                                        op = Some((i, -1));
                                    }
                                    if ui.add_enabled(i + 1 < n, egui::Button::new("↓").small()).clicked() {
                                        op = Some((i, 1));
                                    }
                                    if ui.small_button("×").clicked() {
                                        op = Some((i, 0));
                                    }
                                    ui.label(egui::RichText::new(label(*s)).monospace());
                                });
                            }
                            match op {
                                Some((i, 0)) => {
                                    self.draft.strips.remove(i);
                                }
                                Some((i, d)) => self.draft.strips.swap(i, (i as i32 + d) as usize),
                                None => {}
                            }
                        });
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.set_width(260.0);
                        ui.label(egui::RichText::new("Add (click)").strong());
                        egui::ScrollArea::vertical().id_salt("avail").max_height(360.0).show(ui, |ui| {
                            for s in StripId::all() {
                                if self.draft.strips.contains(&s) {
                                    continue;
                                }
                                if ui.button(egui::RichText::new(label(s)).monospace()).clicked() {
                                    self.draft.strips.push(s);
                                }
                            }
                        });
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command) {
                        result = Some(Some((self.index, self.draft.clone())));
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        result = Some(None);
                    }
                });
            });
        result
    }
}
