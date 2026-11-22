//! Dark pro-console theme.

use eframe::egui::{self, Color32, Visuals};

pub const BG: Color32 = Color32::from_rgb(0x16, 0x17, 0x1a);
pub const PANEL: Color32 = Color32::from_rgb(0x20, 0x22, 0x26);
pub const STRIP: Color32 = Color32::from_rgb(0x2a, 0x2c, 0x31);
pub const STRIP_SEL: Color32 = Color32::from_rgb(0x34, 0x3a, 0x44);
pub const BORDER: Color32 = Color32::from_rgb(0x3a, 0x3d, 0x44);
pub const BUTTON: Color32 = Color32::from_rgb(0x33, 0x36, 0x3c);
pub const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe6, 0xe6);
pub const DIM: Color32 = Color32::from_rgb(0x8a, 0x8d, 0x93);
pub const ACCENT: Color32 = Color32::from_rgb(0x4c, 0xc2, 0xff);
pub const CAP: Color32 = Color32::from_rgb(0x55, 0x59, 0x61);
pub const CAP_HOVER: Color32 = Color32::from_rgb(0x6c, 0x71, 0x7a);
pub const METER_BG: Color32 = Color32::from_rgb(0x0e, 0x0f, 0x11);
pub const METER_GREEN: Color32 = Color32::from_rgb(0x3d, 0xd6, 0x6b);
pub const METER_YELLOW: Color32 = Color32::from_rgb(0xf2, 0xc9, 0x4c);
pub const METER_RED: Color32 = Color32::from_rgb(0xff, 0x4d, 0x4d);
pub const MUTE: Color32 = Color32::from_rgb(0xff, 0x55, 0x55);
pub const SOLO: Color32 = Color32::from_rgb(0xff, 0xc4, 0x3d);
pub const PHANTOM: Color32 = Color32::from_rgb(0xff, 0x8a, 0x3d);
pub const OK: Color32 = Color32::from_rgb(0x3d, 0xd6, 0x6b);
pub const WARN: Color32 = Color32::from_rgb(0xf2, 0xc9, 0x4c);

/// X Air scribble-strip colour (palette lives in xair-proto, shared with the web remote).
pub fn strip_color(code: i32) -> Color32 {
    let [r, g, b] = xair_proto::strip::scribble_rgb(code);
    Color32::from_rgb(r, g, b)
}

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
