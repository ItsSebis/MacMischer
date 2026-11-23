//! RTA window: one big live spectrum of a selectable source (default: main LR), with log
//! frequency and dB axes, smoothing, peak hold and auto gain (see `rta`).

use crate::actions::Action;
use crate::rta::{self, RtaTrace};
use crate::theme;
use eframe::egui::{self, Align2, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use xair_core::MixerState;
use xair_proto::scale::{self, fmt_hz};
use xair_proto::{meters, Param, Value};

pub struct RtaUi {
    pub open: bool,
    /// Shown beside the meter bridge instead of in its own window.
    pub docked: bool,
    /// Auto gain: the dB window follows the signal.
    pub auto_gain: bool,
    pub peak_hold: bool,
    /// Freeze the display (the app stops feeding the trace).
    pub frozen: bool,
    /// The source was switched to LR when the window was first opened this session.
    pub defaulted: bool,
}

impl Default for RtaUi {
    fn default() -> Self {
        RtaUi { open: false, docked: false, auto_gain: true, peak_hold: true, frozen: false, defaulted: false }
    }
}

const MARGIN_L: f32 = 36.0;
const MARGIN_B: f32 = 18.0;
const FREQ_LINES: [f32; 10] = [20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0];

/// What the user asked for in the RTA controls this frame.
#[derive(Default, Clone, Copy)]
pub struct Outcome {
    /// Switch between its own window and the dock beside the meters.
    pub toggle_dock: bool,
    /// Close it (docked only; a window has its own close button).
    pub close: bool,
}

/// The RTA in its own native window (movable, can sit on another screen). Falls back to an
/// in-app floating window when the backend has no multi-window support.
pub fn rta_window(ctx: &egui::Context, ui_state: &mut RtaUi, trace: &RtaTrace, st: &MixerState, fresh: bool, actions: &mut Vec<Action>) -> Outcome {
    let mut out = Outcome::default();
    if !ui_state.open || ui_state.docked {
        return out;
    }
    let builder = egui::ViewportBuilder::default()
        .with_title("MacMischer RTA")
        .with_inner_size([900.0, 440.0])
        .with_min_inner_size([480.0, 260.0]);
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("rta-window"), builder, |ui, _class| {
        // The same body for a native window (root ui) and the embedded fallback (a Window's ui).
        egui::CentralPanel::default().show(ui, |ui| {
            out = contents(ui, ui_state, trace, st, fresh, actions, 160.0);
        });
        if ui.input(|i| i.viewport().close_requested()) {
            ui_state.open = false;
        }
    });
    out
}

/// Controls and plot; shared by the window and the dock. `min_plot_h` is the smallest plot height.
pub fn contents(
    ui: &mut egui::Ui,
    ui_state: &mut RtaUi,
    trace: &RtaTrace,
    st: &MixerState,
    fresh: bool,
    actions: &mut Vec<Action>,
    min_plot_h: f32,
) -> Outcome {
    let out = controls(ui, ui_state, trace, st, actions);
    ui.add_space(2.0);
    let size = ui.available_size().max(Vec2::new(120.0, min_plot_h));
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    plot(ui, rect, ui_state, trace, fresh);
    out
}

fn controls(ui: &mut egui::Ui, ui_state: &mut RtaUi, trace: &RtaTrace, st: &MixerState, actions: &mut Vec<Action>) -> Outcome {
    let mut out = Outcome::default();
    ui.horizontal_wrapped(|ui| {
        let cur = st.get(Param::RtaSource).and_then(Value::as_i32);
        let shown = cur.and_then(meters::rta_source_label).unwrap_or_else(|| "…".into());
        ui.label("Source");
        egui::ComboBox::from_id_salt("rta-source")
            .selected_text(shown)
            .width(90.0)
            .show_ui(ui, |ui| {
                for v in 0..=meters::RTA_SOURCE_MAX {
                    let label = meters::rta_source_label(v).unwrap_or_default();
                    if ui.selectable_label(cur == Some(v), label).clicked() && cur != Some(v) {
                        actions.push(Action::Set(Param::RtaSource, Value::Int(v)));
                    }
                }
            })
            .response
            .on_hover_text("The mixer analyses one source at a time; this is the same setting as on the console, so it changes there too.");
        let pos = st.get(Param::RtaPos).and_then(Value::as_i32);
        ui.label("Tap");
        for (v, label, tip) in [
            (meters::RTA_POS_PRE, "Pre", "Before the source's EQ, filters and fader"),
            (meters::RTA_POS_POST, "Post", "After the source's EQ, filters and fader"),
        ] {
            if ui.selectable_label(pos == Some(v), label).on_hover_text(tip).clicked() && pos != Some(v) {
                actions.push(Action::Set(Param::RtaPos, Value::Int(v)));
            }
        }
        ui.separator();
        ui.checkbox(&mut ui_state.auto_gain, "Auto gain").on_hover_text("Follow the signal level instead of a fixed -100..0 dB window");
        ui.checkbox(&mut ui_state.peak_hold, "Peak hold");
        ui.checkbox(&mut ui_state.frozen, "Freeze");
        let (lo, hi) = trace.range(ui_state.auto_gain);
        ui.label(egui::RichText::new(format!("{lo:.0} … {hi:.0} dB")).size(11.0).color(theme::DIM));
        ui.separator();
        let (label, tip) = if ui_state.docked {
            ("Pop out", "Move the RTA into its own window")
        } else {
            ("Dock", "Show the RTA beside the meter bridge")
        };
        if ui.button(label).on_hover_text(tip).clicked() {
            out.toggle_dock = true;
        }
        if ui_state.docked && ui.button("✕").on_hover_text("Close the RTA").clicked() {
            out.close = true;
        }
    });
    out
}

fn plot(ui: &mut egui::Ui, rect: Rect, ui_state: &RtaUi, trace: &RtaTrace, fresh: bool) {
    let p = ui.painter_at(rect);
    let area = Rect::from_min_max(Pos2::new(rect.left() + MARGIN_L, rect.top() + 4.0), Pos2::new(rect.right() - 6.0, rect.bottom() - MARGIN_B));
    p.rect_filled(area, 4.0, theme::METER_BG);
    let range = trace.range(ui_state.auto_gain);
    let fx = |hz: f32| area.left() + scale::FREQ.from_value(hz) * area.width();
    let fy = |db: f32| area.bottom() - rta::norm(db, range) * area.height();
    let grid = Stroke::new(1.0, theme::BORDER.linear_multiply(0.6));
    let font = FontId::proportional(10.0);

    for hz in FREQ_LINES {
        let x = fx(hz);
        p.line_segment([Pos2::new(x, area.top()), Pos2::new(x, area.bottom())], grid);
        let align = if hz >= 20000.0 { Align2::RIGHT_TOP } else { Align2::CENTER_TOP };
        p.text(Pos2::new(x, area.bottom() + 2.0), align, fmt_hz(hz), font.clone(), theme::DIM);
    }
    let step = if range.1 - range.0 > 80.0 { 20.0 } else { 10.0 };
    let mut db = (range.0 / step).ceil() * step;
    while db <= range.1 {
        let y = fy(db);
        p.line_segment([Pos2::new(area.left(), y), Pos2::new(area.right(), y)], grid);
        p.text(Pos2::new(area.left() - 4.0, y), Align2::RIGHT_CENTER, format!("{db:.0}"), font.clone(), theme::DIM);
        db += step;
    }

    if trace.is_empty() || !(fresh || ui_state.frozen) {
        p.text(area.center(), Align2::CENTER_CENTER, "no RTA data", FontId::proportional(14.0), theme::WARN);
        return;
    }
    let n = trace.smooth.len();
    let fill = theme::ACCENT.linear_multiply(0.30);
    let mut mesh = egui::Mesh::default();
    let mut line: Vec<Pos2> = Vec::with_capacity(n);
    for (i, &v) in trace.smooth.iter().enumerate() {
        let x = fx(meters::rta_band_hz(i));
        let top = Pos2::new(x, fy(v));
        line.push(top);
        mesh.colored_vertex(top, fill);
        mesh.colored_vertex(Pos2::new(x, area.bottom()), fill);
        if i > 0 {
            let k = 2 * i as u32;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    p.add(egui::Shape::mesh(mesh));
    p.add(egui::Shape::line(line, Stroke::new(1.5, theme::ACCENT)));
    if ui_state.peak_hold {
        let peaks: Vec<Pos2> = trace.peak.iter().enumerate().map(|(i, &v)| Pos2::new(fx(meters::rta_band_hz(i)), fy(v))).collect();
        p.add(egui::Shape::line(peaks, Stroke::new(1.0, theme::WARN.linear_multiply(0.8))));
    }
}
