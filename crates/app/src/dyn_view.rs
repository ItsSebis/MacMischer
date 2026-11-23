//! Dynamics detail view: gate and compressor controls with transfer-curve plots.

use crate::actions::Action;
use crate::theme;
use crate::views::Ctx;
use crate::widgets::{self, fmt_db};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use xair_proto::proc::{self, DYN_DETS, DYN_ENVS, DYN_MODES, FILTER_TYPES, GATE_MODES, RATIOS};
use xair_proto::scale::{self, fmt_hz, fmt_ms};
use xair_proto::{Param, Proc, StripId, Value};

const PLOT: f32 = 140.0;

/// Minimum width of one section (plot + two bar columns at their narrowest bars).
const SECTION_MIN_W: f32 = 500.0;
/// Width of the A/B + Presets column to the left of the sections.
const AB_COL_W: f32 = 180.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sub {
    Gate,
    Comp,
}

/// `avail` is the width the view may use (the caller measured it outside any scroll area).
/// Gate and compressor sit side by side; only when that does not fit are they split into two
/// sub-tabs, and if even one section does not fit the detail panel scrolls sideways.
pub fn dyn_view(ui: &mut Ui, c: &mut Ctx, s: StripId, avail: f32) {
    let gate = proc::has_gate(s);
    let dynamics = proc::has_dyn(s);
    let both = gate && dynamics;
    let avail = (avail - AB_COL_W).max(0.0);
    let side_by_side = both && avail >= 2.0 * SECTION_MIN_W + 30.0;
    crate::views::ab_buttons(ui, c);
    if both && !side_by_side {
        let id = egui::Id::unique("dyn-sub");
        let mut sub = ui.data(|d| d.get_temp::<bool>(id)).map_or(Sub::Gate, |g| if g { Sub::Gate } else { Sub::Comp });
        // The tabs take the place of the section heading, so the split costs no height.
        let w = avail.clamp(SECTION_MIN_W, 700.0);
        ui.allocate_ui(Vec2::new(w, 200.0), |ui| {
            ui.set_width(w);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut sub, Sub::Gate, "Gate");
                    ui.selectable_value(&mut sub, Sub::Comp, "Compressor");
                });
                match sub {
                    Sub::Gate => gate_section(ui, c, s, w, false),
                    Sub::Comp => comp_section(ui, c, s, w, false),
                }
            });
        });
        ui.data_mut(|d| d.insert_temp(id, sub == Sub::Gate));
        return;
    }
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        let w = if both { (avail - 30.0) / 2.0 } else { avail.clamp(SECTION_MIN_W, 700.0) };
        if gate {
            ui.allocate_ui(Vec2::new(w, 200.0), |ui| gate_section(ui, c, s, w, true));
            ui.separator();
        }
        if dynamics {
            ui.allocate_ui(Vec2::new(w, 200.0), |ui| comp_section(ui, c, s, w, true));
        }
    });
}

fn set_f(c: &mut Ctx, s: StripId, p: Proc, x: f32) {
    let x = match p.steps() {
        Some(n) => scale::steps_quantize(x, n),
        None => x,
    };
    c.actions.push(Action::Set(Param::Proc(s, p), Value::Float(x)));
}

fn set_i(c: &mut Ctx, s: StripId, p: Proc, v: i32) {
    c.actions.push(Action::Set(Param::Proc(s, p), Value::Int(v)));
}

fn on_toggle(ui: &mut Ui, c: &mut Ctx, s: StripId, p: Proc, label: &str) {
    let on = c.st.proc_i(s, p) != 0;
    if widgets::toggle(ui, Vec2::new(40.0, 20.0), label, on, theme::ACCENT).clicked() {
        set_i(c, s, p, !on as i32);
    }
}

fn small_label(ui: &mut Ui, t: &str) {
    ui.label(RichText::new(t).size(11.0).color(theme::DIM));
}

fn combo(ui: &mut Ui, c: &mut Ctx, s: StripId, p: Proc, items: &[String], width: f32) {
    let cur = (c.st.proc_i(s, p).max(0) as usize).min(items.len() - 1);
    let mut sel = cur;
    egui::ComboBox::from_id_salt(egui::Id::unique(("dyn-combo", s, p)))
        .width(width)
        .selected_text(items[cur].as_str())
        .show_ui(ui, |ui| {
            for (i, it) in items.iter().enumerate() {
                ui.selectable_value(&mut sel, i, it.as_str());
            }
        });
    if sel != cur {
        set_i(c, s, p, sel as i32);
    }
}

fn strs(a: &[&str]) -> Vec<String> {
    a.iter().map(|x| x.to_string()).collect()
}

/// Label + bar + value text. Returns the real-world value.
fn param_bar(ui: &mut Ui, c: &mut Ctx, s: StripId, p: Proc, label: &str, bar_w: f32, fmt: fn(f32) -> String) {
    let x = c.st.proc_f(s, p);
    let v = p.scale().map(|sc| sc.to_value(x)).unwrap_or(0.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.add_sized(Vec2::new(48.0, 16.0), egui::Label::new(RichText::new(label).size(11.0).color(theme::DIM)));
        if let Some(n) = widgets::hbar(ui, Vec2::new(bar_w, 14.0), x, false, egui::Id::unique(("dyn", s, p))) {
            set_f(c, s, p, n);
        }
        ui.add_sized(Vec2::new(52.0, 16.0), egui::Label::new(RichText::new(fmt(v)).size(11.0)));
    });
}

fn f_db(v: f32) -> String {
    format!("{v:.1} dB")
}
fn f_pct(v: f32) -> String {
    format!("{v:.0} %")
}

fn sidechain(ui: &mut Ui, c: &mut Ctx, s: StripId, on: Proc, ty: Proc, f: Proc, bar_w: f32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        small_label(ui, "Filter");
        on_toggle(ui, c, s, on, "On");
        combo(ui, c, s, ty, &strs(&FILTER_TYPES), 56.0);
        param_bar(ui, c, s, f, "Freq", bar_w, fmt_hz);
    });
}

fn bar_w_for(w: f32) -> f32 {
    // two columns beside the plot: each = label 48 + bar + value 52 + gaps
    (((w - PLOT - 24.0) / 2.0) - 48.0 - 52.0 - 12.0).clamp(50.0, 140.0)
}

fn gate_section(ui: &mut Ui, c: &mut Ctx, s: StripId, w: f32, titled: bool) {
    ui.set_width(w);
    ui.vertical(|ui| {
        if titled {
            ui.label(RichText::new("GATE").size(11.0).strong().color(theme::DIM));
        }
        ui.horizontal(|ui| {
            on_toggle(ui, c, s, Proc::GateOn, "On");
            small_label(ui, "Mode");
            combo(ui, c, s, Proc::GateMode, &strs(&GATE_MODES), 60.0);
        });
        let bw = bar_w_for(w);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                param_bar(ui, c, s, Proc::GateThr, "Thresh", bw, f_db);
                param_bar(ui, c, s, Proc::GateRange, "Range", bw, f_db);
                param_bar(ui, c, s, Proc::GateAttack, "Attack", bw, fmt_ms);
            });
            ui.vertical(|ui| {
                param_bar(ui, c, s, Proc::GateHold, "Hold", bw, fmt_ms);
                param_bar(ui, c, s, Proc::GateRelease, "Release", bw, fmt_ms);
            });
            let thr = c.st.proc_value(s, Proc::GateThr).unwrap_or(-40.0);
            let range = c.st.proc_value(s, Proc::GateRange).unwrap_or(30.0);
            let on = c.st.proc_i(s, Proc::GateOn) != 0;
            plot(ui, -80.0, 20.0, on, |x| if x < thr { x - range } else { x });
        });
        sidechain(ui, c, s, Proc::GateFilterOn, Proc::GateFilterType, Proc::GateFilterF, bw);
    });
}

fn comp_section(ui: &mut Ui, c: &mut Ctx, s: StripId, w: f32, titled: bool) {
    ui.set_width(w);
    ui.vertical(|ui| {
        if titled {
            ui.label(RichText::new("COMPRESSOR").size(11.0).strong().color(theme::DIM));
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            on_toggle(ui, c, s, Proc::DynOn, "On");
            small_label(ui, "Mode");
            combo(ui, c, s, Proc::DynMode, &strs(&DYN_MODES), 50.0);
            small_label(ui, "Det");
            combo(ui, c, s, Proc::DynDet, &strs(&DYN_DETS), 50.0);
            small_label(ui, "Env");
            combo(ui, c, s, Proc::DynEnv, &strs(&DYN_ENVS), 44.0);
            on_toggle(ui, c, s, Proc::DynAuto, "Auto");
        });
        let bw = bar_w_for(w);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                param_bar(ui, c, s, Proc::DynThr, "Thresh", bw, f_db);
                ratio_row(ui, c, s);
                param_bar(ui, c, s, Proc::DynKnee, "Knee", bw, |v| format!("{v:.1}"));
                param_bar(ui, c, s, Proc::DynMgain, "Makeup", bw, f_db);
            });
            ui.vertical(|ui| {
                param_bar(ui, c, s, Proc::DynAttack, "Attack", bw, fmt_ms);
                param_bar(ui, c, s, Proc::DynHold, "Hold", bw, fmt_ms);
                param_bar(ui, c, s, Proc::DynRelease, "Release", bw, fmt_ms);
                param_bar(ui, c, s, Proc::DynMix, "Mix", bw, f_pct);
            });
            let thr = c.st.proc_value(s, Proc::DynThr).unwrap_or(-20.0);
            let ri = (c.st.proc_i(s, Proc::DynRatio).max(0) as usize).min(RATIOS.len() - 1);
            let ratio = RATIOS[ri];
            let knee = c.st.proc_value(s, Proc::DynKnee).unwrap_or(0.0) * 6.0;
            let expand = c.st.proc_i(s, Proc::DynMode) == 1;
            let on = c.st.proc_i(s, Proc::DynOn) != 0;
            plot(ui, -60.0, 0.0, on, |x| crate::eqmath::comp_curve(x, thr, ratio, knee, expand));
        });
        let bw2 = bw;
        sidechain(ui, c, s, Proc::DynFilterOn, Proc::DynFilterType, Proc::DynFilterF, bw2);
    });
}

fn ratio_row(ui: &mut Ui, c: &mut Ctx, s: StripId) {
    ui.horizontal(|ui| {
        ui.add_sized(Vec2::new(48.0, 20.0), egui::Label::new(RichText::new("Ratio").size(11.0).color(theme::DIM)));
        let items: Vec<String> = RATIOS.iter().map(|r| format!("{r:.1}:1")).collect();
        combo(ui, c, s, Proc::DynRatio, &items, 60.0);
    });
}

/// Square transfer-curve plot; input dB on x from `lo` to 0, output on y from `lo` to `top`
/// (the x range is `lo..=top` too so the diagonal is 45 degrees).
fn plot(ui: &mut Ui, lo: f32, top: f32, on: bool, f: impl Fn(f32) -> f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(PLOT), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, theme::METER_BG);
    let step = if lo <= -70.0 { 20.0 } else { 10.0 };
    let map = |x: f32, y: f32| {
        let nx = (x - lo) / (top - lo);
        let ny = (y - lo) / (top - lo);
        Pos2::new(rect.left() + nx * rect.width(), rect.bottom() - ny.clamp(0.0, 1.0) * rect.height())
    };
    let grid = Stroke::new(1.0, theme::BORDER.linear_multiply(0.7));
    let mut g = lo + step;
    while g < top - 0.1 {
        p.line_segment([map(g, lo), map(g, top)], grid);
        p.line_segment([map(lo, g), map(top, g)], grid);
        g += step;
    }
    // 0 dB reference
    let zero = Stroke::new(1.0, theme::DIM);
    if top > 0.0 {
        p.line_segment([map(0.0, lo), map(0.0, top)], zero);
        p.line_segment([map(lo, 0.0), map(top, 0.0)], zero);
    }
    p.line_segment([map(lo, lo), map(top, top)], Stroke::new(1.0, theme::CAP));
    let col: Color32 = if on { theme::ACCENT } else { theme::DIM.linear_multiply(0.6) };
    let n = 80;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let x = lo + (top - lo) * i as f32 / n as f32;
            map(x, f(x).max(lo))
        })
        .collect();
    p.add(egui::Shape::line(pts, Stroke::new(2.0, col)));
    p.rect_stroke(rect, 3.0, Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
    p.text(rect.left_top() + Vec2::new(4.0, 2.0), Align2::LEFT_TOP, format!("out {}", fmt_db(top)), FontId::proportional(9.0), theme::DIM);
    p.text(rect.right_bottom() - Vec2::new(4.0, 2.0), Align2::RIGHT_BOTTOM, format!("in {}", fmt_db(lo)), FontId::proportional(9.0), theme::DIM);
}
