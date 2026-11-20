//! Application state, input handling, action dispatcher and top-level layout.

use crate::actions::{nudge_fader, Action};
use crate::bank_editor::BankEditor;
use crate::banks::{self, Bank};
use crate::input::{Entry, NumberEntry};
use crate::keymap::{Binding, Keymap};
use crate::theme;
use crate::views;
use crate::widgets::{Ballistics, UNITY};
use eframe::egui;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use xair_core::{Mixer, MixerInfo, Status};
use xair_proto::{Param, StripId, Value, MIXER_PORT};

enum Conn {
    Discovering(mpsc::Receiver<Vec<(SocketAddr, MixerInfo)>>),
    NotFound,
    Mixer(Mixer),
}

pub struct App {
    ctx: egui::Context,
    conn: Conn,
    _mock: Option<xair_mock::MockMixer>,
    manual_ip: String,
    selected: StripId,
    banks: Vec<Bank>,
    bank: usize,
    ball: HashMap<StripId, Ballistics>,
    entry: NumberEntry,
    keymap: Keymap,
    show_help: bool,
    editor: Option<BankEditor>,
}

impl App {
    pub fn new(ctx: egui::Context, ip: Option<String>, mock: bool) -> App {
        let mut app = App {
            ctx,
            conn: Conn::NotFound,
            _mock: None,
            manual_ip: String::new(),
            selected: StripId::Ch(1),
            banks: banks::defaults(),
            bank: 0,
            ball: HashMap::new(),
            entry: NumberEntry::default(),
            keymap: Keymap::default(),
            show_help: false,
            editor: None,
        };
        // Dev aid for screenshots: MM_SHOW=editor|help opens that dialog on start.
        match std::env::var("MM_SHOW").as_deref() {
            Ok("editor") => app.editor = Some(BankEditor::open(0, &app.banks[0])),
            Ok("help") => app.show_help = true,
            Ok("dca") => app.selected = StripId::Dca(1),
            _ => {}
        }
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

    fn current_bank(&self) -> &Bank {
        &self.banks[self.bank]
    }

    /// The level parameter the keyboard controls for strip `s` (send in a bus-bound bank).
    fn level_param(&self, s: StripId) -> Param {
        match self.current_bank().bus {
            Some(t) if s.has_sends() => Param::SendLevel(s, t),
            _ => Param::Fader(s),
        }
    }

    /// Translate keyboard events into actions.
    fn keyboard(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if ctx.egui_wants_keyboard_input() || self.editor.is_some() {
            return;
        }
        let now = Instant::now();
        let b = self.current_bank();
        let positional = b.positional.then(|| b.strips.len().min(99) as u8);
        self.entry.set_positional(positional);
        let events = ctx.input(|i| i.events.clone());
        for ev in events {
            match ev {
                egui::Event::Text(t) => {
                    for c in t.chars() {
                        match self.entry.char(c, now) {
                            Some(Some(e)) => actions.push(entry_action(e)),
                            Some(None) => {}
                            None if c == '?' => self.show_help = !self.show_help,
                            None => actions.extend(self.keymap.lookup(Binding::Char(c))),
                        }
                    }
                }
                egui::Event::Key { key, pressed: true, modifiers, .. } => match key {
                    egui::Key::Enter => actions.extend(self.entry.enter().map(entry_action)),
                    egui::Key::Escape => {
                        self.entry.cancel();
                        self.show_help = false;
                    }
                    _ => {
                        if let Some(a) = self.keymap.lookup(Binding::Key(key, modifiers.into())) {
                            actions.push(a);
                        }
                    }
                },
                _ => {}
            }
        }
        actions.extend(self.entry.poll(now).map(entry_action));
        if self.entry.active() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        // Tab must not move egui focus around.
        ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
    }

    fn apply(&mut self, actions: Vec<Action>) {
        let Conn::Mixer(m) = &self.conn else { return };
        for a in actions {
            let s = self.selected;
            match a {
                Action::Set(p, v) => m.set(p, v),
                Action::Select(s) => self.selected = m.state().link_leader(s),
                Action::SelectPosition(n) => {
                    let shown = m.state().display_strips(&self.banks[self.bank].strips);
                    if let Some(s) = shown.get(n as usize - 1) {
                        self.selected = *s;
                    }
                }
                Action::SelectBank(i) => {
                    if i < self.banks.len() {
                        self.bank = i;
                    }
                }
                Action::BankStep(d) => {
                    let n = self.banks.len() as i32;
                    self.bank = (self.bank as i32 + d).rem_euclid(n) as usize;
                }
                Action::StripStep(d) => {
                    let strips = &m.state().display_strips(&self.banks[self.bank].strips);
                    if strips.is_empty() {
                        continue;
                    }
                    let n = strips.len() as i32;
                    let i = match strips.iter().position(|x| *x == s) {
                        Some(i) => (i as i32 + d).clamp(0, n - 1),
                        None if d < 0 => n - 1,
                        None => 0,
                    };
                    self.selected = strips[i as usize];
                }
                Action::Nudge(db) => {
                    let p = self.level_param(s);
                    let cur = m.state().get(p).and_then(Value::as_f32).unwrap_or(0.0);
                    m.set(p, Value::Float(nudge_fader(cur, db)));
                }
                Action::Unity => m.set(self.level_param(s), Value::Float(UNITY)),
                Action::PanNudge(d) => {
                    if s.has_pan() {
                        let v = (m.state().pan(s) + d).clamp(0.0, 1.0);
                        m.set(Param::Pan(s), Value::Float(v));
                    }
                }
                Action::ToggleMute => {
                    let on = m.state().on(s);
                    m.set(Param::On(s), Value::Int(!on as i32));
                }
                Action::ToggleSolo => {
                    let on = m.state().soloed(s);
                    set_solo(m, s, !on);
                }
                Action::SetSolo(s, on) => set_solo(m, s, on),
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
                if let Conn::Discovering(_) = self.conn {
                    ui.spinner();
                    ui.label("Searching for X Air mixers…");
                    return;
                }
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
            });
        });
    }

    fn bank_tabs(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal_centered(|ui| {
            let mut edit = None;
            let mut delete = None;
            for (i, b) in self.banks.iter().enumerate() {
                let hash = if b.positional { "  #" } else { "" };
                let mut label = egui::RichText::new(format!("F{}  {}{hash}", i + 1, b.name));
                if b.bus.is_some() {
                    label = label.color(theme::SOLO);
                }
                let r = ui.selectable_label(i == self.bank, label);
                if r.clicked() {
                    actions.push(Action::SelectBank(i));
                }
                if r.double_clicked() {
                    edit = Some(i);
                }
                r.context_menu(|ui| {
                    if ui.button("Edit bank…").clicked() {
                        edit = Some(i);
                    }
                    if ui.button("Delete bank").clicked() {
                        delete = Some(i);
                    }
                });
            }
            if ui.button(" + ").on_hover_text("New bank").clicked() {
                self.banks.push(Bank::new("New bank", []));
                edit = Some(self.banks.len() - 1);
                self.bank = self.banks.len() - 1;
            }
            if let Some(i) = delete {
                if self.banks.len() > 1 {
                    self.banks.remove(i);
                    self.bank = self.bank.min(self.banks.len() - 1);
                }
            }
            if let Some(i) = edit {
                self.editor = Some(BankEditor::open(i, &self.banks[i]));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("?").on_hover_text("Keyboard shortcuts").clicked() {
                    self.show_help = !self.show_help;
                }
                if let Some(t) = self.entry.display() {
                    ui.label(egui::RichText::new(t).size(18.0).monospace().color(theme::ACCENT));
                }
            });
        });
    }
}

/// Solo a strip; linked partners follow (solo isn't mirrored by `Mixer::set`).
fn set_solo(m: &Mixer, s: StripId, on: bool) {
    let partner = m.state().linked_partner(s);
    for x in std::iter::once(s).chain(partner) {
        m.set(Param::Solo(x.solo_index()), Value::Int(on as i32));
    }
}

fn entry_action(e: Entry) -> Action {
    match e {
        Entry::Select(s) => Action::Select(s),
        Entry::Position(n) => Action::SelectPosition(n),
        Entry::Unity => Action::Unity,
    }
}

fn parse_addr(s: &str) -> Option<SocketAddr> {
    let s = s.trim();
    s.parse().ok().or_else(|| format!("{s}:{MIXER_PORT}").parse().ok())
}

fn help_window(ctx: &egui::Context, open: &mut bool) {
    egui::Window::new("Keyboard")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            egui::Grid::new("help").num_columns(2).spacing([24.0, 8.0]).show(ui, |ui| {
                for (k, d) in Keymap::help() {
                    ui.label(egui::RichText::new(*k).monospace().color(theme::ACCENT));
                    ui.label(*d);
                    ui.end_row();
                }
            });
        });
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if !matches!(self.conn, Conn::Mixer(_)) {
            self.connection_screen(ui);
            return;
        }
        let ctx = ui.ctx().clone();
        let mut actions = Vec::new();
        self.keyboard(&ctx, &mut actions);
        // Tabs need &mut self; render them before borrowing mixer state.
        egui::Panel::bottom("banks").exact_size(40.0).show(ui, |ui| self.bank_tabs(ui, &mut actions));

        let Conn::Mixer(mixer) = &self.conn else { unreachable!() };
        let status = mixer.status();
        let st_guard = mixer.state();
        let meters = mixer.meters().clone();
        let st = &*st_guard;
        let bank = &self.banks[self.bank];
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
                    ui.label(
                        egui::RichText::new(format!("{}  {}  fw {}  {}", i.name, i.model, i.firmware, mixer.addr().ip()))
                            .color(theme::DIM),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let idx = st.snap_index();
                    if idx > 0 {
                        ui.label(format!("Scene {idx}: {}", st.snap_name(idx as u8)));
                    }
                    if let Some(t) = bank.bus {
                        let dest = t.strip();
                        ui.label(
                            egui::RichText::new(format!("SENDS ON FADER → {}", st.name(dest)))
                                .strong()
                                .color(theme::strip_color(st.color(dest))),
                        );
                    }
                });
            });
        });
        egui::Panel::top("bridge").exact_size(84.0).show(ui, |ui| views::meter_bridge(ui, &mut c));
        egui::Panel::top("detail").exact_size(230.0).show(ui, |ui| {
            ui.add_space(6.0);
            views::detail(ui, &mut c);
        });
        egui::Panel::right("main").exact_size(views::STRIP_W + 16.0).show(ui, |ui| {
            ui.add_space(6.0);
            views::master_section(ui, &mut c);
            let h = ui.available_height() - 6.0;
            // In a monitor bank the right strip is that bus's master.
            let master = bank.bus.map(|t| t.strip()).unwrap_or(StripId::Main);
            views::strip(ui, &mut c, master, h, None, None);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            let h = ui.available_height();
            if bank.strips.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(egui::RichText::new("Empty bank — double-click its tab to add strips").color(theme::DIM));
                });
                return;
            }
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    for (i, &s) in st.display_strips(&bank.strips).iter().enumerate() {
                        let pos = bank.positional.then_some(i + 1);
                        views::strip(ui, &mut c, s, h - 4.0, bank.bus, pos);
                    }
                });
            });
        });
        let names: HashMap<StripId, String> = StripId::all().into_iter().map(|s| (s, st.name(s))).collect();
        drop(st_guard);

        if let Some(ed) = &mut self.editor {
            if let Some(result) = ed.show(&ctx, &names) {
                if let Some((i, bank)) = result {
                    self.banks[i] = bank;
                }
                self.editor = None;
            }
        }
        help_window(&ctx, &mut self.show_help);
        self.apply(actions);
        // Keep meters animating smoothly between network packets.
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}
