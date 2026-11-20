//! Dark pro-console theme.

use eframe::egui::{self, Color32, Visuals};

pub const BG: Color32 = Color32::from_rgb(0x1a, 0x1b, 0x1e);
pub const PANEL: Color32 = Color32::from_rgb(0x23, 0x25, 0x29);
pub const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe6, 0xe6);
pub const DIM: Color32 = Color32::from_rgb(0x8a, 0x8d, 0x93);
pub const ACCENT: Color32 = Color32::from_rgb(0x4c, 0xc2, 0xff);

pub fn apply(ctx: &egui::Context) {
    let mut v = Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = ACCENT.linear_multiply(0.5);
    v.hyperlink_color = ACCENT;
    ctx.set_visuals(v);
}
