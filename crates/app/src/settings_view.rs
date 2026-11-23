//! Settings window: Remote and Safety tabs.

use crate::banks::Bank;
use crate::prefs::Prefs;
use crate::theme;
use eframe::egui;
use std::collections::HashMap;
use xair_proto::StripId;
use xair_remote::config::RemoteConfig;
use xair_remote::RemoteStatus;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Tab {
    #[default]
    Remote,
    Safety,
}

#[derive(Default)]
pub struct SettingsUi {
    pub open: bool,
    pub tab: Tab,
}

impl SettingsUi {
    /// Show the window on `tab`; if it is already showing that tab, close it.
    pub fn toggle(&mut self, tab: Tab) {
        self.open = !(self.open && self.tab == tab);
        self.tab = tab;
    }
}

/// Returns true if a preference changed (caller saves prefs).
#[allow(clippy::too_many_arguments)]
pub fn settings_window(
    ctx: &egui::Context,
    ui_state: &mut SettingsUi,
    prefs: &mut Prefs,
    remote_cfg: &mut RemoteConfig,
    remote_status: RemoteStatus,
    clients: usize,
    banks: &[Bank],
    current_bank: usize,
    names: &HashMap<StripId, String>,
) -> bool {
    if !ui_state.open {
        return false;
    }
    let mut changed = false;
    let max_h = ctx.content_rect().height() * 0.85;
    let mut open = ui_state.open;
    egui::Window::new("Settings")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .max_height(max_h)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut ui_state.tab, Tab::Remote, "Remote");
                ui.selectable_value(&mut ui_state.tab, Tab::Safety, "Safety");
            });
            ui.separator();
            egui::ScrollArea::vertical().max_height(max_h - 90.0).auto_shrink([false, true]).show(ui, |ui| {
                ui.set_width(620.0);
                match ui_state.tab {
                    Tab::Remote => {
                        crate::remote_view::remote_panel(ui, remote_cfg, remote_status, clients, banks, current_bank, names)
                    }
                    Tab::Safety => changed |= safety(ui, prefs),
                }
            });
        });
    ui_state.open = open;
    changed
}

fn safety(ui: &mut egui::Ui, prefs: &mut Prefs) -> bool {
    let before = (prefs.safe_mode, prefs.disable_scroll);
    ui.label(egui::RichText::new("Mode").strong());
    ui.radio_value(&mut prefs.safe_mode, true, "Safe: scene recall and preset loads need a confirming second click");
    ui.radio_value(&mut prefs.safe_mode, false, "Simple: no confirmations");
    ui.add_space(10.0);
    ui.label(egui::RichText::new("Scrolling").strong());
    ui.checkbox(&mut prefs.disable_scroll, "Disable scroll wheel on faders, sends, pan and EQ bands");
    ui.label(
        egui::RichText::new("Prevents accidental level changes from the mouse wheel or trackpad. Dragging and keys still work.")
            .size(11.0)
            .color(theme::DIM),
    );
    before != (prefs.safe_mode, prefs.disable_scroll)
}
