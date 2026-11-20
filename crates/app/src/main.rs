//! MacMischer — keyboard-first controller for Behringer XR18.
//!
//! `macmischer [--ip <addr>] [--mock]` — without options the mixer is discovered by broadcast.

mod actions;
mod app;
mod bank_editor;
mod banks;
mod connect;
mod dyn_view;
mod eq_view;
mod eqmath;
mod input;
mod keymap;
mod prefs;
mod scenes;
mod routing_view;
mod session;
mod show;
mod theme;
mod views;
mod widgets;

use eframe::egui;

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu=warn,naga=warn,eframe=warn,egui_wgpu=warn".into()),
        )
        .init();
    let args: Vec<String> = std::env::args().collect();
    let ip = args.iter().position(|a| a == "--ip").and_then(|i| args.get(i + 1)).cloned();
    let mock = args.iter().any(|a| a == "--mock");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("MacMischer")
            .with_app_id("macmischer")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1000.0, 640.0])
            .with_icon(std::sync::Arc::new(egui::IconData {
                // Generated from assets/icon/icon.svg by `cargo run -p icon-gen`.
                rgba: include_bytes!("../../../assets/icon/icon-256.rgba").to_vec(),
                width: 256,
                height: 256,
            })),
        ..Default::default()
    };
    eframe::run_native(
        "MacMischer",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(app::App::new(cc.egui_ctx.clone(), ip, mock)))
        }),
    )
}
