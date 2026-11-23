//! Web remote settings window: enable/port/PIN, status, URL + QR code, and page editor.

use crate::banks::Bank;
use crate::theme;
use crate::widgets::{icon_button, Icon};
use eframe::egui;
use std::collections::HashMap;
use xair_proto::{SendTo, StripId};
use xair_remote::config::{sendto_to_str, PageConfig, RemoteConfig};
use xair_remote::RemoteStatus;

/// The "Remote" tab of the settings window: enable/port/PIN, status, URL + QR code, page editor.
pub fn remote_panel(
    ui: &mut egui::Ui,
    cfg: &mut RemoteConfig,
    status: RemoteStatus,
    clients: usize,
    banks: &[Bank],
    current_bank: usize,
    names: &HashMap<StripId, String>,
) {
    let bank = banks.get(current_bank);
    header(ui, cfg, &status, clients);
    if let RemoteStatus::Running { port } = status {
        ui.separator();
        urls(ui, port);
    }
    ui.separator();
    pages(ui, cfg, bank, names);
}

fn header(ui: &mut egui::Ui, cfg: &mut RemoteConfig, status: &RemoteStatus, clients: usize) {
    ui.checkbox(&mut cfg.enabled, "Enable web remote");
    egui::Grid::new("remote-hdr").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label("Port");
        ui.add(egui::DragValue::new(&mut cfg.port).range(1024..=65535));
        ui.end_row();
        ui.label("PIN");
        ui.add(egui::TextEdit::singleline(&mut cfg.pin).hint_text("none").desired_width(100.0));
        cfg.pin.retain(|c| c.is_ascii_digit());
        cfg.pin.truncate(8);
        ui.end_row();
    });
    let (text, color) = match status {
        RemoteStatus::Off => ("Off".to_string(), theme::DIM),
        RemoteStatus::Running { port } => (
            format!("Running on port {port} - {clients} phone{} connected", if clients == 1 { "" } else { "s" }),
            theme::OK,
        ),
        RemoteStatus::Error(e) => (e.clone(), theme::MUTE),
    };
    ui.label(egui::RichText::new(text).color(color));
}

fn urls(ui: &mut egui::Ui, port: u16) {
    let urls: Vec<String> =
        xair_remote::lan_addresses().iter().map(|ip| format!("http://{ip}:{port}/")).collect();
    let Some(first) = urls.first() else {
        ui.label(egui::RichText::new("No LAN address found.").color(theme::WARN));
        return;
    };
    ui.horizontal_top(|ui| {
        qr(ui, first, 180.0);
        ui.vertical(|ui| {
            for (i, u) in urls.iter().enumerate() {
                ui.horizontal(|ui| {
                    let t = if i == 0 {
                        egui::RichText::new(u).monospace().strong().size(16.0)
                    } else {
                        egui::RichText::new(u).monospace().color(theme::DIM)
                    };
                    ui.add(egui::Label::new(t).selectable(true));
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(u.clone());
                    }
                });
            }
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "Scan with the phone camera. On iPhone: Share, then Add to Home Screen for a full-screen app.",
                    )
                    .color(theme::DIM)
                    .size(11.0),
                )
                .wrap(),
            );
        });
    });
}

fn qr(ui: &mut egui::Ui, url: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, egui::Color32::WHITE);
    let Ok(code) = qrcode::QrCode::new(url.as_bytes()) else { return };
    let w = code.width();
    let cell = size / (w + 8) as f32;
    for (i, c) in code.to_colors().into_iter().enumerate() {
        if c == qrcode::Color::Dark {
            let (x, y) = ((i % w) as f32 + 4.0, (i / w) as f32 + 4.0);
            let r = egui::Rect::from_min_size(
                rect.min + egui::vec2(x * cell, y * cell),
                egui::Vec2::splat(cell + 0.5),
            );
            p.rect_filled(r, 0.0, egui::Color32::BLACK);
        }
    }
}

fn pages(ui: &mut egui::Ui, cfg: &mut RemoteConfig, bank: Option<&Bank>, names: &HashMap<StripId, String>) {
    ui.label(egui::RichText::new("Pages").strong());
    if cfg.pages.is_empty() {
        ui.label(egui::RichText::new("No pages yet - create one from a fader bank.").color(theme::DIM));
    }
    let mut remove = None;
    for (i, page) in cfg.pages.iter_mut().enumerate() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.push_id(i, |ui| page_ui(ui, page, bank, names, || remove = Some(i)));
        });
    }
    if let Some(i) = remove {
        cfg.pages.remove(i);
    }
    ui.horizontal(|ui| {
        if ui.add_enabled(bank.is_some(), egui::Button::new("New page from current bank")).clicked() {
            if let Some(b) = bank {
                cfg.pages.push(PageConfig::new(&b.name, b.strips.clone(), b.bus));
            }
        }
        if ui.button("New empty page").clicked() {
            cfg.pages.push(PageConfig::new("New page", Vec::new(), None));
        }
    });
}

fn page_ui(
    ui: &mut egui::Ui,
    page: &mut PageConfig,
    bank: Option<&Bank>,
    names: &HashMap<StripId, String>,
    mut on_remove: impl FnMut(),
) {
    let nm = |s: &StripId| names.get(s).filter(|n| !n.is_empty()).cloned().unwrap_or_else(|| s.short());
    egui::Grid::new("page").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label("Name");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut page.name).desired_width(300.0));
            if icon_button(ui, Icon::Remove, true).on_hover_text("Delete page").clicked() {
                on_remove();
            }
        });
        ui.end_row();

        ui.label("Strips");
        let summary = if page.strips.is_empty() {
            "(none)".to_string()
        } else {
            page.strips.iter().map(nm).collect::<Vec<_>>().join(", ")
        };
        ui.add(egui::Label::new(egui::RichText::new(summary).color(theme::DIM)).truncate());
        ui.end_row();

        ui.label("Faders control");
        let bus_name = |t: SendTo| format!("Sends to {}", nm(&t.strip()));
        let cur = page.bus.map_or("Channel levels".to_string(), bus_name);
        egui::ComboBox::from_id_salt("bus").selected_text(cur).show_ui(ui, |ui| {
            ui.selectable_value(&mut page.bus, None, "Channel levels");
            for t in SendTo::all() {
                ui.selectable_value(&mut page.bus, Some(t), format!("{} ({})", bus_name(t), sendto_to_str(t)));
            }
        });
        ui.end_row();

        ui.label("Level cap");
        ui.horizontal(|ui| {
            let mut limit = page.max_db.is_some();
            if ui.checkbox(&mut limit, "Limit").changed() {
                page.max_db = limit.then_some(0.0);
            }
            if let Some(db) = &mut page.max_db {
                ui.add(egui::Slider::new(db, -30.0..=10.0).suffix(" dB").fixed_decimals(1));
            }
        });
        ui.end_row();
    });
    ui.checkbox(&mut page.meters, "Meters");
    ui.horizontal(|ui| {
        if ui.add_enabled(bank.is_some(), egui::Button::new("Set strips from current bank")).clicked() {
            if let Some(b) = bank {
                page.strips = b.strips.clone();
            }
        }
    });
    egui::CollapsingHeader::new(format!("Edit strips ({})", page.strips.len())).id_salt("strips").show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            for s in StripId::all() {
                let on = page.strips.contains(&s);
                let txt = egui::RichText::new(s.short()).monospace().color(if on { egui::Color32::BLACK } else { theme::TEXT });
                let btn = egui::Button::new(txt).fill(if on { theme::ACCENT } else { theme::BUTTON });
                if ui.add(btn).on_hover_text(nm(&s)).clicked() {
                    if on {
                        page.strips.retain(|x| *x != s);
                    } else {
                        // Keep console order.
                        let all = StripId::all();
                        let pos = |x: &StripId| all.iter().position(|a| a == x).unwrap_or(usize::MAX);
                        page.strips.push(s);
                        page.strips.sort_by_key(pos);
                    }
                }
            }
        });
    });
}
