//! MacMischer — keyboard-first controller for Behringer XR18.
//!
//! `macmischer [--ip <addr>] [--mock]` — without options the mixer is discovered by broadcast.

mod actions;
mod banks;
mod theme;
mod views;
mod widgets;

use actions::Action;
use banks::Bank;
use eframe::egui;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::mpsc;
use std::time::Duration;
use widgets::Ballistics;
use xair_core::{Mixer, MixerInfo, Status};
use xair_proto::{StripId, MIXER_PORT};

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
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1000.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "MacMischer",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(App::new(cc.egui_ctx.clone(), ip, mock)))
        }),
    )
}

enum Conn {
    Discovering(mpsc::Receiver<Vec<(SocketAddr, MixerInfo)>>),
    NotFound,
    Mixer(Mixer),
}

struct App {
    ctx: egui::Context,
    conn: Conn,
    _mock: Option<xair_mock::MockMixer>,
    manual_ip: String,
    selected: StripId,
    banks: Vec<Bank>,
    bank: usize,
    ball: HashMap<StripId, Ballistics>,
}

impl App {
    fn new(ctx: egui::Context, ip: Option<String>, mock: bool) -> App {
        let mut app = App {
            ctx,
            conn: Conn::NotFound,
            _mock: None,
            manual_ip: String::new(),
            selected: StripId::Ch(1),
            banks: banks::defaults(),
            bank: 0,
            ball: HashMap::new(),
        };
        if mock {
            let m = xair_mock::MockMixer::start();
            app.connect(m.addr());
            app._mock = Some(m);
        } else if let Some(addr) = ip.as_deref().and_then(parse_addr) {
            app.connect(addr);
        } else {
            app.start_discovery();
        }
        app
    }

    fn start_discovery(&mut self) {
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(xair_core::discover(&[], Duration::from_millis(800)));
            ctx.request_repaint();
        });
        self.conn = Conn::Discovering(rx);
    }

    fn connect(&mut self, addr: SocketAddr) {
        let mixer = Mixer::connect(addr);
        let ctx = self.ctx.clone();
        mixer.set_repaint(move || ctx.request_repaint());
        self.conn = Conn::Mixer(mixer);
    }

    fn apply(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Set(p, v) => {
                    if let Conn::Mixer(m) = &self.conn {
                        m.set(p, v);
                    }
                }
                Action::Select(s) => self.selected = s,
                Action::SelectBank(i) => self.bank = i.min(self.banks.len() - 1),
            }
        }
    }

    fn connection_screen(&mut self, ui: &mut egui::Ui) {
        if let Conn::Discovering(rx) = &self.conn {
            match rx.try_recv() {
                Ok(found) => match found.first() {
                    Some((addr, info)) => {
                        tracing::info!("discovered {} at {addr}", info.name);
                        let addr = *addr;
                        self.connect(addr);
                        return;
                    }
                    None => self.conn = Conn::NotFound,
                },
                Err(mpsc::TryRecvError::Empty) => {}
                Err(_) => self.conn = Conn::NotFound,
            }
        }
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                match self.conn {
                    Conn::Discovering(_) => {
                        ui.spinner();
                        ui.label("Searching for X Air mixers…");
                    }
                    _ => {
                        ui.heading("No mixer found");
                        ui.label("Is the XR18 switched on and on the same network?");
                        ui.add_space(8.0);
                        if ui.button("Search again").clicked() {
                            self.start_discovery();
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.add_space(ui.available_width() / 2.0 - 130.0);
                            ui.label("IP:");
                            let r = ui.text_edit_singleline(&mut self.manual_ip);
                            let go = ui.button("Connect").clicked()
                                || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                            if go {
                                if let Some(a) = parse_addr(&self.manual_ip) {
                                    self.connect(a);
                                }
                            }
                        });
                    }
                }
            });
        });
    }
}

fn parse_addr(s: &str) -> Option<SocketAddr> {
    let s = s.trim();
    s.parse().ok().or_else(|| format!("{s}:{MIXER_PORT}").parse().ok())
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let Conn::Mixer(mixer) = &self.conn else {
            self.connection_screen(ui);
            return;
        };
        let status = mixer.status();
        let st_guard = mixer.state();
        let meters = mixer.meters().clone();
        let st = &*st_guard;
        let mut actions = Vec::new();
        let mut c = views::Ctx {
            st,
            meters: &meters,
            ball: &mut self.ball,
            selected: self.selected,
            actions: &mut actions,
        };

        egui::Panel::top("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("MacMischer").strong());
                ui.separator();
                let (txt, col) = match status {
                    Status::Connected => ("connected".to_string(), theme::OK),
                    Status::Syncing { done, total } => (format!("syncing {done}/{total}"), theme::WARN),
                    Status::Connecting => ("connecting…".to_string(), theme::WARN),
                    Status::Lost => ("connection lost — retrying".to_string(), theme::MUTE),
                };
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 4.5, col);
                ui.colored_label(col, txt);
                if let Some(i) = &st.info {
                    ui.label(egui::RichText::new(format!("{}  {}  fw {}  {}", i.name, i.model, i.firmware, mixer.addr().ip())).color(theme::DIM));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let idx = st.snap_index();
                    if idx > 0 {
                        ui.label(format!("Scene {idx}: {}", st.snap_name(idx as u8)));
                    }
                });
            });
        });
        egui::Panel::top("bridge").exact_size(84.0).show(ui, |ui| views::meter_bridge(ui, &mut c));
        egui::Panel::top("detail").exact_size(230.0).show(ui, |ui| {
            ui.add_space(6.0);
            views::detail(ui, &mut c);
        });
        egui::Panel::bottom("banks").exact_size(40.0).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                for (i, b) in self.banks.iter().enumerate() {
                    let label = format!("F{}  {}", i + 1, b.name);
                    if ui.selectable_label(i == self.bank, label).clicked() {
                        c.actions.push(Action::SelectBank(i));
                    }
                }
            });
        });
        egui::Panel::right("main").exact_size(views::STRIP_W + 16.0).show(ui, |ui| {
            ui.add_space(6.0);
            let h = ui.available_height() - 6.0;
            views::strip(ui, &mut c, StripId::Main, h, None);
        });
        let bank = &self.banks[self.bank];
        egui::CentralPanel::default().show(ui, |ui| {
            let h = ui.available_height();
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    for &s in &bank.strips {
                        views::strip(ui, &mut c, s, h - 4.0, bank.bus);
                    }
                });
            });
        });
        drop(st_guard);
        self.apply(actions);
        // Keep meters animating smoothly even between network packets.
        ui.ctx().request_repaint_after(Duration::from_millis(33));
    }
}
