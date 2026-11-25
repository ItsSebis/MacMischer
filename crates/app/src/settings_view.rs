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
    Appearance,
    Updates,
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
    check_at_launch: &mut bool,
    remote_cfg: &mut RemoteConfig,
    remote_status: RemoteStatus,
    clients: usize,
    banks: &[Bank],
    current_bank: usize,
    names: &HashMap<StripId, String>,
    bg_error: Option<&str>,
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
                ui.selectable_value(&mut ui_state.tab, Tab::Appearance, "Appearance");
                ui.selectable_value(&mut ui_state.tab, Tab::Updates, "Updates");
            });
            ui.separator();
            egui::ScrollArea::vertical().max_height(max_h - 90.0).auto_shrink([false, true]).show(ui, |ui| {
                ui.set_width(620.0);
                match ui_state.tab {
                    Tab::Remote => {
                        crate::remote_view::remote_panel(ui, remote_cfg, remote_status, clients, banks, current_bank, names)
                    }
                    Tab::Safety => changed |= safety(ui, prefs),
                    Tab::Appearance => changed |= appearance(ui, prefs, bg_error),
                    Tab::Updates => {
                        changed |= ui.checkbox(check_at_launch, "Check for updates at launch").changed();
                        ui.label(
                            egui::RichText::new(concat!("MacMischer v", env!("CARGO_PKG_VERSION"))).size(11.0).color(theme::DIM),
                        );
                    }
                }
            });
        });
    ui_state.open = open;
    changed
}

fn appearance(ui: &mut egui::Ui, prefs: &mut Prefs, error: Option<&str>) -> bool {
    let mut changed = false;
    let bg = &mut prefs.background;
    ui.label(egui::RichText::new("Background image").strong());
    let mut text = bg.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut text).hint_text("Path to a PNG or JPEG (or drop a file on the window)").desired_width(440.0));
        if r.lost_focus() {
            let t = text.trim();
            let new = (!t.is_empty()).then(|| std::path::PathBuf::from(t));
            if new != bg.path {
                bg.path = new;
                changed = true;
            }
        }
        if ui.add_enabled(bg.path.is_some(), egui::Button::new("Clear")).clicked() {
            bg.path = None;
            changed = true;
        }
    });
    if let Some(e) = error {
        ui.label(egui::RichText::new(e).size(11.0).color(theme::WARN));
    }
    ui.add_space(6.0);
    changed |= ui.add(egui::Slider::new(&mut bg.opacity, 0.0..=1.0).text("Image opacity")).changed();
    ui.label(
        egui::RichText::new("The grey panels are layered on top; 0 is plain grey, 1 shows the image undimmed.")
            .size(11.0)
            .color(theme::DIM),
    );
    changed
}

fn safety(ui: &mut egui::Ui, prefs: &mut Prefs) -> bool {
    let before = (prefs.safe_mode, prefs.disable_scroll, prefs.quit_on_close);
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
    ui.add_space(10.0);
    ui.label(egui::RichText::new("Closing the window").strong());
    let mut keep = !prefs.quit_on_close;
    if ui.checkbox(&mut keep, "Keep running in the menu bar / tray while the web remote is on").changed() {
        prefs.quit_on_close = !keep;
    }
    ui.label(
        egui::RichText::new("The remote keeps serving until you choose Quit from the tray icon. With the remote off, closing the window quits.")
            .size(11.0)
            .color(theme::DIM),
    );
    before != (prefs.safe_mode, prefs.disable_scroll, prefs.quit_on_close)
}
