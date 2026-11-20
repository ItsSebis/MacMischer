//! Application state, input handling, action dispatcher and top-level layout.

use crate::actions::{nudge_fader, Action};
use crate::bank_editor::BankEditor;
use crate::banks::Bank;
use crate::scenes::ScenesUi;
use crate::session::Session;
use crate::input::{Entry, NumberEntry};
use crate::keymap::{Binding, Keymap};
use crate::theme;
use crate::views;
use crate::widgets::{Ballistics, UNITY};
use eframe::egui;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use crate::connect::{parse_addr, Finder};
use xair_core::{Mixer, Status};
use xair_proto::{Param, StripId, Value};

enum Conn {
    Finding(Finder),
    Mixer(Mixer),
}

pub struct App {
    ctx: egui::Context,
    conn: Conn,
    _mock: Option<xair_mock::MockMixer>,
    selected: StripId,
    session: Session,
    ball: HashMap<StripId, Ballistics>,
    entry: NumberEntry,
    keymap: Keymap,
    show_help: bool,
    editor: Option<BankEditor>,
    scenes: ScenesUi,
    show_routing: bool,
    /// Lock mode: only mutes, solo, navigation and the main fader respond.
    locked: bool,
    /// Short status message (e.g. "Locked").
    flash: Option<(String, Instant)>,
}

impl App {
    pub fn new(ctx: egui::Context, ip: Option<String>, mock: bool) -> App {
        let session = Session::load();
        let remembered = session.prefs.last_mixer.as_deref().and_then(parse_addr);
        let mut app = App {
            ctx,
            conn: Conn::Finding(Finder::new(remembered, true)),
            _mock: None,
            selected: StripId::Ch(1),
            session,
            ball: HashMap::new(),
            entry: NumberEntry::default(),
            keymap: Keymap::default(),
            show_help: false,
            editor: None,
            scenes: ScenesUi::default(),
            show_routing: false,
            locked: false,
            flash: None,
        };
        // Dev aid for screenshots: MM_SHOW=editor|help|dca|scenes|routing opens that dialog on start.
        match std::env::var("MM_SHOW").as_deref() {
            Ok("editor") => app.editor = Some(BankEditor::open(0, &app.session.banks[0])),
            Ok("help") => app.show_help = true,
            Ok("dca") => app.selected = StripId::Dca(1),
            Ok("scenes") => app.scenes.open = true,
            Ok("routing") => app.show_routing = true,
            _ => {}
        }
        if mock {
            let m = xair_mock::MockMixer::start();
            app.connect_to(m.addr(), false);
            app._mock = Some(m);
        } else if let Some(addr) = ip.as_deref().and_then(parse_addr) {
            app.connect(addr);
        }
        app
    }

    /// Drop the current connection and show the mixer list ("Switch mixer").
    fn switch_mixer(&mut self) {
        let current = match &self.conn {
            Conn::Mixer(m) => Some(m.addr()),
            Conn::Finding(_) => None,
        };
        self.conn = Conn::Finding(Finder::new(current, false));
    }

    fn connect(&mut self, addr: SocketAddr) {
        self.connect_to(addr, true);
    }

    /// `remember` = store as last mixer in the preferences.
    fn connect_to(&mut self, addr: SocketAddr, remember: bool) {
        if remember {
            self.session.prefs.last_mixer = Some(addr.to_string());
            self.session.prefs.save();
        }
        let mixer = Mixer::connect(addr);
        let ctx = self.ctx.clone();
        mixer.set_repaint(move || ctx.request_repaint());
        self.conn = Conn::Mixer(mixer);
    }

    fn current_bank(&self) -> &Bank {
        &self.session.banks[self.session.bank]
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
            if self.locked && !a.allowed_when_locked(s) {
                self.flash = Some(("Locked — Ctrl/Cmd+L to unlock".into(), Instant::now()));
                continue;
            }
            match a {
                Action::Set(p, v) => m.set(p, v),
                Action::Select(s) => self.selected = m.state().link_leader(s),
                Action::SelectPosition(n) => {
                    let shown = m.state().display_strips(&self.session.banks[self.session.bank].strips);
                    if let Some(s) = shown.get(n as usize - 1) {
                        self.selected = *s;
                    }
                }
                Action::SelectBank(i) => {
                    if i < self.session.banks.len() {
                        self.session.bank = i;
                    }
                }
                Action::BankStep(d) => {
                    let n = self.session.banks.len() as i32;
                    self.session.bank = (self.session.bank as i32 + d).rem_euclid(n) as usize;
                }
                Action::StripStep(d) => {
                    let strips = &m.state().display_strips(&self.session.banks[self.session.bank].strips);
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
                Action::RecallScene(i) => {
                    m.load_snapshot(i);
                    self.flash = Some((format!("Recalled scene {i}"), Instant::now()));
                }
                Action::SaveScene(i, name) => {
                    m.save_snapshot(i);
                    m.set(Param::SnapName(i), Value::Str(name));
                    self.flash = Some((format!("Saved scene {i}"), Instant::now()));
                }
                Action::RenameScene(i, name) => m.set(Param::SnapName(i), Value::Str(name)),
                Action::ToggleLock => self.locked = !self.locked,
                Action::OpenScenes => self.scenes.open = !self.scenes.open,
                Action::OpenRouting => self.show_routing = !self.show_routing,
            }
        }
    }

    fn bank_tabs(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal_centered(|ui| {
            self.session.menu(ui);
            ui.separator();
            let mut edit = None;
            let mut delete = None;
            for (i, b) in self.session.banks.iter().enumerate() {
                let hash = if b.positional { "  #" } else { "" };
                let mut label = egui::RichText::new(format!("F{}  {}{hash}", i + 1, b.name));
                if b.bus.is_some() {
                    label = label.color(theme::SOLO);
                }
                let r = ui.selectable_label(i == self.session.bank, label);
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
                self.session.banks.push(Bank::new("New bank", []));
                edit = Some(self.session.banks.len() - 1);
                self.session.bank = self.session.banks.len() - 1;
            }
            if let Some(i) = delete {
                if self.session.banks.len() > 1 {
                    self.session.banks.remove(i);
                    self.session.bank = self.session.bank.min(self.session.banks.len() - 1);
                }
            }
            if let Some(i) = edit {
                self.editor = Some(BankEditor::open(i, &self.session.banks[i]));
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
    fn on_exit(&mut self) {
        self.session.save_now();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Conn::Finding(f) = &mut self.conn {
            if let Some(addr) = f.ui(ui) {
                self.connect(addr);
            }
            return;
        }
        let ctx = ui.ctx().clone();
        let mut actions = Vec::new();
        self.keyboard(&ctx, &mut actions);
        // Tabs need &mut self; render them before borrowing mixer state.
        egui::Panel::bottom("banks").exact_size(40.0).show(ui, |ui| self.bank_tabs(ui, &mut actions));

        let mut switch = false;
        let mut prefs_changed = false;
        let locked = self.locked;
        let Conn::Mixer(mixer) = &self.conn else { unreachable!() };
        let status = mixer.status();
        let st_guard = mixer.state();
        let meters = mixer.meters().clone();
        let st = &*st_guard;
        let bank = &self.session.banks[self.session.bank];
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
                let title = match &st.info {
                    Some(i) => format!("{}  {}  fw {}  {}", i.name, i.model, i.firmware, mixer.addr().ip()),
                    None => mixer.addr().to_string(),
                };
                ui.menu_button(egui::RichText::new(title).color(theme::DIM), |ui| {
                    if ui.button("Switch mixer…").clicked() {
                        switch = true;
                    }
                    if ui.button("Re-read mixer state").clicked() {
                        mixer.resync();
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Lock + mode
                    let lock_txt = if locked {
                        egui::RichText::new(" LOCKED ").strong().color(egui::Color32::BLACK).background_color(theme::MUTE)
                    } else {
                        egui::RichText::new("Lock")
                    };
                    if ui.button(lock_txt).on_hover_text("Ctrl/Cmd+L — only mutes and the main fader work while locked").clicked() {
                        c.actions.push(Action::ToggleLock);
                    }
                    let safe = &mut self.session.prefs.safe_mode;
                    let before = *safe;
                    ui.selectable_value(safe, true, "Safe").on_hover_text("Scene recall needs a confirming second click");
                    ui.selectable_value(safe, false, "Simple").on_hover_text("No confirmations");
                    if *safe != before {
                        prefs_changed = true;
                    }
                    ui.separator();
                    if ui.button("Routing").on_hover_text("Ctrl/Cmd+R").clicked() {
                        c.actions.push(Action::OpenRouting);
                    }
                    let idx = st.snap_index();
                    let scene = if idx > 0 { format!("Scene {idx}: {}", st.snap_name(idx as u8)) } else { "Scenes".into() };
                    if ui.button(scene).on_hover_text("Ctrl/Cmd+K").clicked() {
                        c.actions.push(Action::OpenScenes);
                    }
                    if let Some((msg, t)) = &self.flash {
                        if t.elapsed() < Duration::from_secs(2) {
                            ui.colored_label(theme::WARN, msg);
                        }
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
        let st_for_windows = st;
        self.scenes.window(&ctx, st_for_windows, self.session.prefs.safe_mode, &mut actions);
        crate::routing_view::routing_window(&ctx, &mut self.show_routing, st_for_windows, self.locked, &mut actions);
        drop(st_guard);

        if let Some(ed) = &mut self.editor {
            if let Some(result) = ed.show(&ctx, &names) {
                if let Some((i, bank)) = result {
                    self.session.banks[i] = bank;
                }
                self.editor = None;
            }
        }
        help_window(&ctx, &mut self.show_help);
        self.apply(actions);
        if switch {
            self.switch_mixer();
        }
        if prefs_changed {
            self.session.prefs.save();
        }
        self.session.autosave(&ctx);
        // Keep meters animating smoothly between network packets.
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}
