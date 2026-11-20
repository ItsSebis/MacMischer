//! Custom console widgets: meter, fader, pan bar, toggle buttons.

use crate::theme;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Stroke, StrokeKind,
    Ui, Vec2,
};
use std::time::Instant;
use xair_proto::fader;

/// Unity position as stored by the mixer (767/1023).
pub const UNITY: f32 = 767.0 / 1023.0;

/// Meter taper: 0 dBFS at the top, same curve as the fader law shifted by 10 dB.
pub fn meter_pos(db: f32) -> f32 {
    fader::from_db(db + 10.0)
}

/// Visual ballistics: fast attack, smooth release, peak hold.
#[derive(Clone, Copy)]
pub struct Ballistics {
    level: f32,
    peak: f32,
    peak_at: Instant,
    last: Instant,
}

impl Default for Ballistics {
    fn default() -> Self {
        let now = Instant::now();
        Ballistics { level: -128.0, peak: -128.0, peak_at: now, last: now }
    }
}

impl Ballistics {
    /// Feed a new raw dB value; returns (display level, peak).
    pub fn update(&mut self, db: f32) -> (f32, f32) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;
        self.level = if db >= self.level { db } else { (self.level - 24.0 * dt).max(db) };
        if db >= self.peak || (now - self.peak_at).as_secs_f32() > 1.5 {
            self.peak = db.max(self.level);
            self.peak_at = now;
        }
        (self.level, self.peak)
    }
}

/// Paint a vertical meter into `rect`.
pub fn paint_meter(ui: &Ui, rect: Rect, level_db: f32, peak_db: f32) {
    let p = ui.painter();
    p.rect_filled(rect, 2.0, theme::METER_BG);
    let h = rect.height();
    let fill_top = rect.bottom() - meter_pos(level_db) * h;
    // Segmented colour: green up to -12, yellow to -3, red above.
    let seg = |from_db: f32, to_db: f32, c: Color32| {
        let y0 = rect.bottom() - meter_pos(from_db) * h;
        let y1 = (rect.bottom() - meter_pos(to_db) * h).max(fill_top);
        if y1 < y0 {
            p.rect_filled(Rect::from_x_y_ranges(rect.x_range(), y1..=y0), 0.0, c);
        }
    };
    seg(-128.0, -12.0, theme::METER_GREEN);
    seg(-12.0, -3.0, theme::METER_YELLOW);
    seg(-3.0, 10.0, theme::METER_RED);
    if peak_db > -90.0 {
        let y = rect.bottom() - meter_pos(peak_db) * h;
        let c = if peak_db > -3.0 { theme::METER_RED } else { theme::TEXT };
        p.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], Stroke::new(1.5, c));
    }
}

/// Vertical fader. Returns the new 0..1 position if the user changed it.
/// Drag (Shift = fine), scroll wheel / two-finger, double-click = unity.
pub fn fader(ui: &mut Ui, size: Vec2, value: f32, accent: Color32, id: egui::Id) -> Option<f32> {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let resp = ui.interact(rect, id, Sense::click_and_drag()).union(resp);
    let track = Rect::from_center_size(rect.center(), Vec2::new(6.0, rect.height() - 24.0));
    let p = ui.painter();

    // Scale ticks
    for (db, label) in [(10.0, "+10"), (5.0, "5"), (0.0, "0"), (-10.0, "10"), (-20.0, "20"), (-30.0, "30"), (-50.0, "50")] {
        let y = track.bottom() - fader::from_db(db) * track.height();
        let x0 = track.right() + 4.0;
        p.line_segment([Pos2::new(x0, y), Pos2::new(x0 + 5.0, y)], Stroke::new(1.0, theme::DIM));
        p.text(Pos2::new(x0 + 7.0, y), Align2::LEFT_CENTER, label, FontId::proportional(9.0), theme::DIM);
    }
    p.rect_filled(track, 3.0, theme::METER_BG);
    let fill = Rect::from_x_y_ranges(track.x_range(), (track.bottom() - value * track.height())..=track.bottom());
    p.rect_filled(fill, 3.0, accent.linear_multiply(0.6));

    // Cap
    let y = track.bottom() - value * track.height();
    let cap = Rect::from_center_size(Pos2::new(track.center().x, y), Vec2::new(34.0, 18.0));
    let cap_col = if resp.dragged() || resp.hovered() { theme::CAP_HOVER } else { theme::CAP };
    p.rect_filled(cap, 3.0, cap_col);
    p.line_segment([Pos2::new(cap.left() + 3.0, y), Pos2::new(cap.right() - 3.0, y)], Stroke::new(2.0, accent));

    let mut new = None;
    if resp.double_clicked() {
        new = Some(UNITY);
    } else if resp.dragged() {
        let fine = if ui.input(|i| i.modifiers.shift) { 0.1 } else { 1.0 };
        let dv = -resp.drag_delta().y / track.height() * fine;
        if dv != 0.0 {
            new = Some((value + dv).clamp(0.0, 1.0));
        }
    } else if resp.hovered() {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        if dy != 0.0 {
            new = Some((value + dy * 0.0015).clamp(0.0, 1.0));
        }
    }
    new.map(fader::quantize)
}

/// Horizontal bar 0..1. `centered` = bipolar (pan: fills from the middle, double-click → 0.5);
/// otherwise fills from the left (send level: double-click → unity).
pub fn hbar(ui: &mut Ui, size: Vec2, value: f32, centered: bool, id: egui::Id) -> Option<f32> {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let p = ui.painter();
    p.rect_filled(rect, 3.0, theme::METER_BG);
    let x = rect.left() + value * rect.width();
    let from = if centered { rect.center().x } else { rect.left() };
    let bar = Rect::from_x_y_ranges(from.min(x)..=from.max(x).max(from + 1.0), rect.y_range());
    let col = if resp.hovered() || resp.dragged() { theme::ACCENT } else { theme::ACCENT.linear_multiply(0.7) };
    p.rect_filled(bar, 2.0, col);
    if centered {
        let cx = rect.center().x;
        p.line_segment([Pos2::new(cx, rect.top()), Pos2::new(cx, rect.bottom())], Stroke::new(1.0, theme::DIM));
    } else {
        let ux = rect.left() + UNITY * rect.width();
        p.line_segment([Pos2::new(ux, rect.top()), Pos2::new(ux, rect.bottom())], Stroke::new(1.0, theme::DIM));
    }
    let new = if resp.double_clicked() {
        Some(if centered { 0.5 } else { UNITY })
    } else if resp.dragged() {
        let fine = if ui.input(|i| i.modifiers.shift) { 0.1 } else { 1.0 };
        Some(value + resp.drag_delta().x / rect.width() * fine)
    } else if resp.hovered() {
        let d = ui.input(|i| i.smooth_scroll_delta);
        let d = if d.x != 0.0 { d.x } else { d.y };
        (d != 0.0).then_some(value + d * 0.0015)
    } else {
        None
    };
    new.map(|v| v.clamp(0.0, 1.0)).filter(|v| *v != value)
}

/// Small latching button (mute/solo/phantom). Returns true when clicked.
pub fn toggle(ui: &mut Ui, size: Vec2, label: &str, active: bool, on_color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    let (bg, fg) = if active { (on_color, Color32::BLACK) } else { (theme::BUTTON, theme::DIM) };
    let bg = if resp.hovered() { bg.gamma_multiply(1.2) } else { bg };
    p.rect_filled(rect, CornerRadius::same(3), bg);
    p.rect_stroke(rect, 3.0, Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(11.0), fg);
    resp
}

/// Format a fader dB value for display.
pub fn fmt_db(db: f32) -> String {
    if db == f32::NEG_INFINITY || db < -89.5 {
        "-∞".into()
    } else if db.abs() < 0.05 {
        "0.0".into()
    } else {
        format!("{db:+.1}")
    }
}

/// Pan 0..1 as L50 / C / R50.
pub fn fmt_pan(v: f32) -> String {
    let x = ((v - 0.5) * 200.0).round() as i32;
    match x {
        0 => "C".into(),
        x if x < 0 => format!("L{}", -x),
        x => format!("R{x}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_db(f32::NEG_INFINITY), "-∞");
        assert_eq!(fmt_db(0.0), "0.0");
        assert_eq!(fmt_db(-0.0098), "0.0");
        assert_eq!(fmt_db(-8.0), "-8.0");
        assert_eq!(fmt_pan(0.5), "C");
        assert_eq!(fmt_pan(0.0), "L100");
        assert_eq!(fmt_pan(0.75), "R50");
        assert!((fader::to_db(UNITY)).abs() < 0.05);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Icon {
    Up,
    Down,
    Remove,
}

/// Small square button with a painted icon (no font glyph dependency).
pub fn icon_button(ui: &mut Ui, icon: Icon, enabled: bool) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(20.0), sense);
    let p = ui.painter();
    let bg = if enabled && resp.hovered() { theme::CAP_HOVER } else { theme::BUTTON };
    p.rect_filled(rect, 3.0, bg);
    let fg = if enabled { theme::TEXT } else { theme::BORDER };
    let c = rect.center();
    let r = 5.0;
    match icon {
        Icon::Up | Icon::Down => {
            let s = if matches!(icon, Icon::Up) { -1.0 } else { 1.0 };
            let pts = vec![
                Pos2::new(c.x - r, c.y - s * r * 0.5),
                Pos2::new(c.x + r, c.y - s * r * 0.5),
                Pos2::new(c.x, c.y + s * r * 0.6),
            ];
            p.add(egui::Shape::convex_polygon(pts, fg, Stroke::NONE));
        }
        Icon::Remove => {
            let st = Stroke::new(1.8, if enabled { theme::MUTE } else { fg });
            p.line_segment([c + Vec2::new(-r, -r), c + Vec2::new(r, r)], st);
            p.line_segment([c + Vec2::new(-r, r), c + Vec2::new(r, -r)], st);
        }
    }
    resp
}
