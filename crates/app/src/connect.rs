//! Finding a mixer: background discovery, auto-pick (remembered / single), list, manual IP.

use crate::theme;
use eframe::egui;
use std::net::SocketAddr;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use xair_core::MixerInfo;
use xair_proto::MIXER_PORT;

const RETRY: Duration = Duration::from_secs(3);

type Found = xair_core::Discovery;

pub struct Finder {
    searching: Option<mpsc::Receiver<Found>>,
    found: Vec<(SocketAddr, MixerInfo)>,
    /// Why the last search could not even send (shown with a hint).
    send_error: Option<String>,
    /// Last successfully used mixer (unicast-probed too, in case broadcast is filtered).
    remembered: Option<SocketAddr>,
    /// Pick automatically (remembered, or the only one found). Off for "Switch mixer".
    auto: bool,
    next_search: Instant,
    manual_ip: String,
    error: Option<String>,
}

impl Finder {
    pub fn new(remembered: Option<SocketAddr>, auto: bool) -> Finder {
        Finder {
            searching: None,
            found: Vec::new(),
            send_error: None,
            remembered,
            auto,
            next_search: Instant::now(),
            manual_ip: remembered.map(|a| a.ip().to_string()).unwrap_or_default(),
            error: None,
        }
    }

    fn search(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let extra: Vec<_> = self.remembered.map(|a| a.ip()).into_iter().collect();
        std::thread::spawn(move || {
            let _ = tx.send(xair_core::discover_report(&extra, Duration::from_millis(800)));
            ctx.request_repaint();
        });
        self.searching = Some(rx);
    }

    /// Advance discovery; returns a mixer to connect to when one is chosen.
    fn poll(&mut self, ctx: &egui::Context) -> Option<SocketAddr> {
        if let Some(rx) = &self.searching {
            match rx.try_recv() {
                Ok(found) => {
                    self.searching = None;
                    self.found = found.found;
                    self.send_error = found.send_error;
                    self.next_search = Instant::now() + RETRY;
                    if self.auto {
                        let remembered = self.found.iter().find(|(a, _)| Some(*a) == self.remembered);
                        if let Some((a, _)) = remembered {
                            return Some(*a);
                        }
                        if let [(a, _)] = self.found.as_slice() {
                            return Some(*a);
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(_) => self.searching = None,
            }
        } else if self.found.is_empty() && Instant::now() >= self.next_search {
            // Nothing found yet: keep looking (switching the mixer on is enough).
            self.search(ctx);
        }
        ctx.request_repaint_after(Duration::from_millis(250));
        None
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<SocketAddr> {
        let ctx = ui.ctx().clone();
        let mut pick = self.poll(&ctx);
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.22);
                ui.label(egui::RichText::new("MacMischer").size(28.0).strong());
                ui.add_space(16.0);
                if self.found.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(ui.available_width() / 2.0 - 110.0);
                        ui.spinner();
                        ui.label("Looking for X Air mixers…");
                    });
                    ui.label(
                        egui::RichText::new("Switch the mixer on — it connects automatically when it appears.")
                            .color(theme::DIM),
                    );
                    if let Some(e) = &self.send_error {
                        ui.add_space(8.0);
                        ui.colored_label(theme::MUTE, format!("Cannot reach the network: {e}"));
                        if cfg!(target_os = "macos") {
                            ui.label(
                                egui::RichText::new(
                                    "macOS: allow MacMischer under System Settings → Privacy & Security → Local Network, then restart the app.",
                                )
                                .color(theme::DIM),
                            );
                        }
                    }
                } else {
                    ui.label(egui::RichText::new("Choose a mixer").size(16.0));
                    ui.add_space(6.0);
                    for (addr, info) in &self.found {
                        let text = format!("{}   {}   fw {}   {}", info.name, info.model, info.firmware, addr.ip());
                        let mut b = egui::Button::new(egui::RichText::new(text).size(15.0)).min_size(egui::vec2(420.0, 36.0));
                        if Some(*addr) == self.remembered {
                            b = b.stroke(egui::Stroke::new(1.5, theme::ACCENT));
                        }
                        if ui.add(b).clicked() {
                            pick = Some(*addr);
                        }
                    }
                    ui.add_space(6.0);
                    if ui.button("Search again").clicked() {
                        self.found.clear();
                        self.search(&ctx);
                    }
                }
                ui.add_space(24.0);
                ui.horizontal(|ui| {
                    ui.add_space(ui.available_width() / 2.0 - 150.0);
                    ui.label("Or enter IP:");
                    let r = ui.add(egui::TextEdit::singleline(&mut self.manual_ip).desired_width(140.0));
                    let go = ui.button("Connect").clicked()
                        || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                    if go {
                        match parse_addr(&self.manual_ip) {
                            Some(a) => pick = Some(a),
                            None => self.error = Some(format!("“{}” is not an IP address", self.manual_ip)),
                        }
                    }
                });
                if let Some(e) = &self.error {
                    ui.colored_label(theme::MUTE, e);
                }
            });
        });
        pick
    }
}

pub fn parse_addr(s: &str) -> Option<SocketAddr> {
    let s = s.trim();
    s.parse().ok().or_else(|| format!("{s}:{MIXER_PORT}").parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_addresses() {
        assert_eq!(parse_addr("10.5.5.48"), Some("10.5.5.48:10024".parse().unwrap()));
        assert_eq!(parse_addr(" 10.5.5.48:9000 "), Some("10.5.5.48:9000".parse().unwrap()));
        assert_eq!(parse_addr("mixer"), None);
    }
}
