//! EQ detail view: response graph with draggable band handles + compact band controls.
//!
//! Graph: drag a numbered handle (x = frequency, y = gain), scroll on it = Q,
//! double-click = gain 0. The low-cut handle (channels) sits on the bottom edge.

use crate::actions::Action;
use crate::eqmath::{self, Band, BandType};
use crate::theme;
use crate::views::Ctx;
use crate::widgets;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use xair_proto::proc::{self, EqField, Proc};
use xair_proto::scale::{self, fmt_hz};
use xair_proto::{Param, StripId, Value};

const DB_RANGE: f32 = 18.0;
const BAND_COLORS: [Color32; 6] = [
    Color32::from_rgb(0xff, 0x8a, 0x3d),
    Color32::from_rgb(0xf2, 0xc9, 0x4c),
    Color32::from_rgb(0x3d, 0xd6, 0x6b),
    Color32::from_rgb(0x4c, 0xc2, 0xff),
    Color32::from_rgb(0xb0, 0x7c, 0xff),
    Color32::from_rgb(0xff, 0x6b, 0xa8),
];

fn band_color(b: u8) -> Color32 {
    BAND_COLORS[(b as usize - 1) % BAND_COLORS.len()]
}

/// Read the bands of strip `s` in real units.
fn bands(c: &Ctx, s: StripId) -> Vec<Band> {
    (1..=proc::eq_bands(s))
        .map(|b| {
            let v = |f: EqField| c.st.proc_value(s, Proc::Eq(b, f)).unwrap_or(0.0);
            Band {
                kind: BandType::from_code(c.st.proc_i(s, Proc::Eq(b, EqField::Type))),
                hz: v(EqField::F),
                gain_db: v(EqField::G),
                q: v(EqField::Q),
            }
        })
        .collect()
}

/// Send a float processing parameter given its real value.
fn set_real(c: &mut Ctx, s: StripId, p: Proc, real: f32) {
    if let Some(sc) = p.scale() {
        let mut x = sc.from_value(real);
        if let Some(n) = p.steps() {
            x = scale::steps_quantize(x, n);
        }
        c.actions.push(Action::Set(Param::Proc(s, p), Value::Float(x)));
    }
}

fn set_int(c: &mut Ctx, s: StripId, p: Proc, v: i32) {
    c.actions.push(Action::Set(Param::Proc(s, p), Value::Int(v)));
}

pub fn eq_view(ui: &mut Ui, c: &mut Ctx, s: StripId) {
    let n = proc::eq_bands(s);
    let eq_on = c.st.proc_i(s, Proc::EqOn) != 0;
    let hpf = proc::has_hpf(s).then(|| {
        (c.st.proc_i(s, Proc::HpOn) != 0, c.st.proc_value(s, Proc::Hpf).unwrap_or(20.0))
    });
    let bands = bands(c, s);

    ui.horizontal_top(|ui| {
        // Left: on/off switches
        ui.vertical(|ui| {
            ui.set_width(70.0);
            if widgets::toggle(ui, Vec2::new(64.0, 26.0), "EQ", eq_on, theme::OK).clicked() {
                set_int(c, s, Proc::EqOn, !eq_on as i32);
            }
            if let Some((on, hz)) = hpf {
                ui.add_space(6.0);
                if widgets::toggle(ui, Vec2::new(64.0, 26.0), "LOW CUT", on, theme::ACCENT).clicked() {
                    set_int(c, s, Proc::HpOn, !on as i32);
                }
                ui.label(egui::RichText::new(fmt_hz(hz)).monospace().size(11.0));
            }
        });

        // Graph
        let (rect, _) = ui.allocate_exact_size(Vec2::new(540.0, 205.0), Sense::hover());
        graph(ui, c, s, rect, &bands, eq_on, hpf);

        ui.add_space(8.0);
        // Band controls
        egui::Grid::new(("eqgrid", s)).num_columns(5).spacing([8.0, 5.0]).show(ui, |ui| {
            for h in ["", "Type", "Freq", "Gain", "Q"] {
                ui.label(egui::RichText::new(h).size(10.0).color(theme::DIM));
            }
            ui.end_row();
            for b in 1..=n {
                let band = bands[b as usize - 1];
                ui.label(egui::RichText::new(format!("{b}")).strong().color(band_color(b)));
                let mut t = c.st.proc_i(s, Proc::Eq(b, EqField::Type));
                let before = t;
                egui::ComboBox::from_id_salt(("eqtype", s, b))
                    .width(62.0)
                    .selected_text(proc::EQ_TYPES.get(t as usize).copied().unwrap_or("?"))
                    .show_ui(ui, |ui| {
                        for (i, name) in proc::EQ_TYPES.iter().enumerate() {
                            ui.selectable_value(&mut t, i as i32, *name);
                        }
                    });
                if t != before {
                    set_int(c, s, Proc::Eq(b, EqField::Type), t);
                }
                for (field, text, w) in [
                    (EqField::F, fmt_hz(band.hz), 190.0),
                    (EqField::G, format!("{:+.1} dB", band.gain_db), 160.0),
                    (EqField::Q, format!("{:.1}", band.q), 120.0),
                ] {
                    let p = Proc::Eq(b, field);
                    ui.horizontal(|ui| {
                        let x = c.st.proc_f(s, p);
                        let enabled = field != EqField::G || band.kind.uses_gain();
                        ui.add_enabled_ui(enabled, |ui| {
                            if let Some(nx) = widgets::hbar(ui, Vec2::new(w - 60.0, 14.0), x, field == EqField::G, egui::Id::unique(("eqbar", s, b, field))) {
                                let nx = p.steps().map_or(nx, |st| scale::steps_quantize(nx, st));
                                c.actions.push(Action::Set(Param::Proc(s, p), Value::Float(nx)));
                            }
                        });
                        ui.label(egui::RichText::new(text).monospace().size(10.5));
                    });
                }
                ui.end_row();
            }
        });
    });
}

fn graph(ui: &mut Ui, c: &mut Ctx, s: StripId, rect: Rect, bands: &[Band], eq_on: bool, hpf: Option<(bool, f32)>) {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 4.0, theme::METER_BG);
    let fx = |hz: f32| rect.left() + scale::FREQ.from_value(hz) * rect.width();
    let fy = |db: f32| rect.center().y - (db / DB_RANGE) * (rect.height() / 2.0 - 6.0);
    let hz_at = |x: f32| scale::FREQ.to_value(((x - rect.left()) / rect.width()).clamp(0.0, 1.0));
    let db_at = |y: f32| ((rect.center().y - y) / (rect.height() / 2.0 - 6.0) * DB_RANGE).clamp(-15.0, 15.0);

    // Grid
    let grid = Stroke::new(1.0, theme::BORDER.linear_multiply(0.6));
    for hz in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0] {
        let x = fx(hz);
        p.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], grid);
        p.text(Pos2::new(x + 2.0, rect.bottom() - 2.0), Align2::LEFT_BOTTOM, fmt_hz(hz), FontId::proportional(9.0), theme::DIM);
    }
    for db in [-12.0, -6.0, 0.0, 6.0, 12.0] {
        let y = fy(db);
        let st = if db == 0.0 { Stroke::new(1.0, theme::DIM.linear_multiply(0.7)) } else { grid };
        p.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], st);
        p.text(Pos2::new(rect.left() + 2.0, y - 1.0), Align2::LEFT_BOTTOM, format!("{db:+}"), FontId::proportional(9.0), theme::DIM);
    }

    // Curves: individual bands faint, sum bright.
    let freqs = eqmath::log_freqs(rect.width() as usize / 2);
    let to_pts = |vals: &[f32]| -> Vec<Pos2> {
        freqs.iter().zip(vals).map(|(&f, &v)| Pos2::new(fx(f), fy(v.clamp(-DB_RANGE, DB_RANGE)))).collect()
    };
    for (i, b) in bands.iter().enumerate() {
        let r = eqmath::response(std::slice::from_ref(b), None, &freqs);
        let col = band_color(i as u8 + 1).linear_multiply(if eq_on { 0.35 } else { 0.15 });
        p.add(egui::Shape::line(to_pts(&r), Stroke::new(1.0, col)));
    }
    let active_bands: &[Band] = if eq_on { bands } else { &[] };
    let hp = hpf.and_then(|(on, hz)| on.then_some(hz));
    let total = eqmath::response(active_bands, hp, &freqs);
    let col = if eq_on || hp.is_some() { theme::ACCENT } else { theme::DIM };
    p.add(egui::Shape::line(to_pts(&total), Stroke::new(2.0, col)));
    if !eq_on {
        p.text(rect.right_top() + Vec2::new(-6.0, 4.0), Align2::RIGHT_TOP, "EQ OFF", FontId::proportional(11.0), theme::DIM);
    }

    // Low-cut handle (bottom edge)
    if let Some((on, hz)) = hpf {
        let x = fx(hz);
        let id = egui::Id::unique(("hpf", s));
        let hr = Rect::from_center_size(Pos2::new(x, rect.bottom() - 10.0), Vec2::new(18.0, 18.0));
        let resp = ui.interact(hr, id, Sense::click_and_drag());
        let fill = if on { theme::ACCENT } else { theme::DIM };
        let tri = vec![
            Pos2::new(x - 7.0, rect.bottom() - 3.0),
            Pos2::new(x + 7.0, rect.bottom() - 3.0),
            Pos2::new(x, rect.bottom() - 16.0),
        ];
        p.add(egui::Shape::convex_polygon(tri, fill, Stroke::NONE));
        if resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                set_real(c, s, Proc::Hpf, hz_at(pos.x).clamp(20.0, 400.0));
            }
        }
        if resp.double_clicked() {
            set_int(c, s, Proc::HpOn, !on as i32);
        }
        resp.on_hover_text(format!("Low cut {}  (drag; double-click = on/off)", fmt_hz(hz)));
    }

    // Band handles
    for (i, b) in bands.iter().enumerate() {
        let n = i as u8 + 1;
        let gy = if b.kind.uses_gain() { b.gain_db } else { 0.0 };
        let center = Pos2::new(fx(b.hz), fy(gy));
        let id = egui::Id::unique(("eqh", s, n));
        let resp = ui.interact(Rect::from_center_size(center, Vec2::splat(22.0)), id, Sense::click_and_drag());
        let col = band_color(n);
        let r = if resp.hovered() || resp.dragged() { 10.0 } else { 8.0 };
        p.circle_filled(center, r, if eq_on { col } else { col.linear_multiply(0.4) });
        p.text(center, Align2::CENTER_CENTER, n.to_string(), FontId::proportional(10.0), Color32::BLACK);
        if resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                set_real(c, s, Proc::Eq(n, EqField::F), hz_at(pos.x));
                if b.kind.uses_gain() {
                    set_real(c, s, Proc::Eq(n, EqField::G), db_at(pos.y));
                }
            }
        }
        if resp.hovered() {
            let d = ui.input(|i| i.smooth_scroll_delta.y);
            if d != 0.0 {
                // Scroll up = narrower (higher Q).
                let q = (b.q * (1.0 + d * 0.004)).clamp(0.3, 10.0);
                set_real(c, s, Proc::Eq(n, EqField::Q), q);
            }
        }
        if resp.double_clicked() && b.kind.uses_gain() {
            set_real(c, s, Proc::Eq(n, EqField::G), 0.0);
        }
        resp.on_hover_text(format!(
            "Band {n}: {}  {}  {:+.1} dB  Q {:.1}\ndrag = freq/gain, scroll = Q, double-click = 0 dB",
            proc::EQ_TYPES[(c.st.proc_i(s, Proc::Eq(n, EqField::Type)).clamp(0, 5)) as usize],
            fmt_hz(b.hz),
            b.gain_db,
            b.q
        ));
    }
}
