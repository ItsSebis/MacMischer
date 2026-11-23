//! Application state, input handling, action dispatcher and top-level layout.

use crate::actions::Action;
use crate::console::{Console, Env};
use crate::midi::input::{Frame, MidiInput, MixerView};
use crate::bank_editor::BankEditor;
use crate::banks::Bank;
use crate::scenes::ScenesUi;
use crate::session::Session;
use crate::input::{Entry, NumberEntry};
use crate::keymap::{Binding, Keymap};
use crate::theme;
use crate::views;
use crate::widgets::Ballistics;
use eframe::egui;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use crate::connect::{parse_addr, Finder};
use xair_core::{Mixer, Status};
use xair_proto::StripId;

enum Conn {
    Finding(Finder),
    Mixer(Arc<Mixer>),
}

pub struct App {
    ctx: egui::Context,
    conn: Conn,
    _mock: Option<xair_mock::MockMixer>,
    session: Session,
    console: Console,
    ball: HashMap<StripId, Ballistics>,
    entry: NumberEntry,
    keymap: Keymap,
    show_help: bool,
    editor: Option<BankEditor>,
    scenes: ScenesUi,
    remote: xair_remote::Remote,
    settings: crate::settings_view::SettingsUi,
    midi: crate::midi::MidiService,
    midi_input: MidiInput,
    midi_ui: crate::midi_view::MidiUi,
    view: views::DetailView,
    show_routing: bool,
    show_fx: bool,
    /// EQ view RTA overlay toggle (session only, default off).
    rta_on: bool,
    rta_ui: crate::rta_view::RtaUi,
    rta_trace: crate::rta::RtaTrace,
    /// Height of the meters+detail band as drawn last frame (the RTA dock spans it).
    upper_h: Option<f32>,
    /// Short status message (e.g. "Locked").
    flash: Option<(String, Instant)>,
    /// Release check; independent of the Console and the mixer connection.
    updater: crate::updater::Updater,
    background: crate::background::Background,
}

impl App {
    pub fn new(ctx: egui::Context, ip: Option<String>, mock: bool) -> App {
        let session = Session::load();
        let remembered = session.prefs.last_mixer.as_deref().and_then(parse_addr);
        let mut app = App {
            ctx: ctx.clone(),
            conn: Conn::Finding(Finder::new(remembered, true)),
            _mock: None,
            session,
            console: Console::new(crate::presets::dir()),
            ball: HashMap::new(),
            entry: NumberEntry::default(),
            keymap: Keymap::default(),
            show_help: false,
            editor: None,
            scenes: ScenesUi::default(),
            remote: xair_remote::Remote::new(),
            settings: Default::default(),
            midi: crate::midi::MidiService::new(ctx.clone()),
            midi_input: Default::default(),
            midi_ui: Default::default(),
            view: views::DetailView::Mix,
            show_routing: false,
            show_fx: false,
            rta_on: false,
            rta_ui: Default::default(),
            rta_trace: Default::default(),
            upper_h: None,
            flash: None,
            background: Default::default(),
            updater: {
                let c = ctx.clone();
                let mut u = crate::updater::Updater::production(move || c.request_repaint());
                u.check_at_launch();
                u
            },
        };
        app.rta_ui.docked = app.session.prefs.ui.rta_docked;
        // Dev aid for screenshots: MM_SHOW=editor|help|dca|scenes|routing opens that dialog on start.
        match std::env::var("MM_SHOW").as_deref() {
            Ok("editor") => app.editor = Some(BankEditor::open(0, &app.session.banks.list()[0])),
            Ok("help") => app.show_help = true,
            Ok("dca") => app.console.select(StripId::Dca(1)),
            Ok("scenes") => app.scenes.open = true,
            Ok("routing") => app.show_routing = true,
            Ok("rta") => app.rta_ui.open = true,
            Ok("dyn-docked") => {
                app.rta_ui.open = true;
                app.rta_ui.docked = true;
                app.view = views::DetailView::Dynamics;
            }
            Ok("rta-docked") => {
                app.rta_ui.open = true;
                app.rta_ui.docked = true;
            }
            Ok("remote") => app.settings.toggle(crate::settings_view::Tab::Remote),
            Ok("settings") => app.settings.toggle(crate::settings_view::Tab::Safety),
            Ok("eq") => app.view = views::DetailView::Eq,
            Ok("dyn") => app.view = views::DetailView::Dynamics,
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
        self.console.reset();
        self.rta_on = false;
        self.rta_ui.defaulted = false;
        self.rta_trace.clear();
        self.midi_input.reset();
        let current = match &self.conn {
            Conn::Mixer(m) => Some(m.addr()),
            Conn::Finding(_) => None,
        };
        self.remote.set_mixer(None);
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
        let mixer = Arc::new(Mixer::connect(addr));
        self.remote.set_mixer(Some(mixer.clone()));
        let ctx = self.ctx.clone();
        mixer.set_repaint(move || ctx.request_repaint());
        self.conn = Conn::Mixer(mixer);
    }

    fn current_bank(&self) -> &Bank {
        self.session.banks.current()
    }

    /// Translate keyboard events into actions.
    fn keyboard(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if ctx.egui_wants_keyboard_input() || self.editor.is_some() {
            return;
        }
        let now = Instant::now();
        let b = self.current_bank();
        // Positions count the strips as displayed: a linked pair is one strip.
        let shown = match &self.conn {
            Conn::Mixer(m) => m.state().display_strips(&b.strips).len(),
            Conn::Finding(_) => b.strips.len(),
        };
        let positional = b.positional.then(|| shown.min(99) as u8);
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

    /// Window and bank actions are handled here; everything else goes to the Console.
    fn apply(&mut self, actions: Vec<Action>) {
        let Conn::Mixer(m) = &self.conn else { return };
        let m = m.clone();
        let mut batch = Vec::new();
        for a in actions {
            match a {
                Action::SelectBank(i) => {
                    self.flush(&m, &mut batch);
                    self.session.banks.select(i);
                }
                Action::BankStep(d) => {
                    self.flush(&m, &mut batch);
                    self.session.banks.step(d);
                }
                Action::OpenScenes => self.scenes.open = !self.scenes.open,
                Action::OpenRouting => self.show_routing = !self.show_routing,
                Action::OpenFx => self.show_fx = !self.show_fx,
                Action::OpenRemote => self.settings.toggle(crate::settings_view::Tab::Remote),
                Action::OpenRta => {
                    self.rta_ui.open = !self.rta_ui.open;
                    // The mixer defaults to analysing MON before processing; the first time the window opens, look at main out, post.
                    if self.rta_ui.open && !self.rta_ui.defaulted {
                        self.rta_ui.defaulted = true;
                        batch.push(Action::Set(xair_proto::Param::RtaSource, xair_proto::Value::Int(xair_proto::meters::RTA_SOURCE_LR)));
                        batch.push(Action::Set(xair_proto::Param::RtaPos, xair_proto::Value::Int(xair_proto::meters::RTA_POS_POST)));
                    }
                }
                Action::OpenSettings => self.settings.toggle(crate::settings_view::Tab::Safety),
                Action::OpenMidi => self.midi_ui.open = !self.midi_ui.open,
                Action::SetView(v) => self.view = v,
                other => batch.push(other),
            }
        }
        self.flush(&m, &mut batch);
    }

    /// Hand queued Actions to the Console (in order, against the bank current at this point).
    fn flush(&mut self, m: &Mixer, batch: &mut Vec<Action>) {
        if batch.is_empty() {
            return;
        }
        let bank = self.session.banks.current();
        let env = Env {
            bank_strips: &bank.strips,
            level_bus: bank.bus,
            safe_mode: self.session.prefs.safe_mode,
            now: Instant::now(),
        };
        let out = self.console.apply(m, &env, batch.drain(..));
        if out.recalled {
            self.midi_input.reset();
        }
        if let Some(msg) = out.flash {
            self.flash = Some((msg, Instant::now()));
        }
    }

    /// Handle MIDI messages (Learn, hold, soft takeover, mapping) via [`MidiInput`].
    /// Returns true when the bindings changed (prefs need saving).
    fn midi_input(&mut self, msgs: Vec<crate::midi::MidiMsg>, actions: &mut Vec<Action>) -> bool {
        let Conn::Mixer(m) = &self.conn else { return false };
        let now = Instant::now();
        let resyncing = !matches!(m.status(), Status::Connected) || self.console.resyncing(now);
        let st = m.state();
        let fed = self.midi_input.feed(
            msgs,
            Frame {
                prefs: &mut self.session.prefs.midi,
                learn: &mut self.midi_ui.learn,
                status: self.midi.status(),
                view: &MixerView { st: &st },
                safe_mode: self.session.prefs.safe_mode,
                resyncing,
                now,
            },
        );
        drop(st);
        actions.extend(fed.actions);
        if let Some(text) = fed.error {
            self.flash = Some((text, Instant::now()));
        }
        fed.bindings_changed
    }

    fn bank_tabs(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal_centered(|ui| {
            self.session.menu(ui);
            ui.separator();
            let mut edit = None;
            let mut delete = None;
            for (i, b) in self.session.banks.list().iter().enumerate() {
                let mut label = egui::RichText::new(format!("F{}  {}", i + 1, b.name));
                if b.bus.is_some() {
                    label = label.color(theme::SOLO);
                }
                let r = ui.selectable_label(i == self.session.banks.index(), label);
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
                edit = Some(self.session.banks.add(Bank::new("New bank", [])));
            }
            if let Some(i) = delete {
                self.session.banks.delete(i);
            }
            if let Some(i) = edit {
                self.editor = Some(BankEditor::open(i, &self.session.banks.list()[i]));
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

impl App {
    /// Mirror the Updater's preferences into the session's copy so the next save keeps them.
    fn sync_update_prefs(prefs: &mut crate::prefs::Prefs, updater: &crate::updater::Updater) {
        prefs.updates = updater.prefs().clone();
    }

    /// Non-blocking banner announcing a newer Release; Update now downloads, verifies and installs it.
    /// Dropping an image file on the window sets it as the background.
    fn take_dropped_image(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).next());
        if let Some(p) = dropped.filter(|p| crate::background::is_image(p)) {
            self.session.prefs.background.path = Some(p);
            self.session.prefs.save();
        }
    }

    fn update_banner(&mut self, ui: &mut egui::Ui) {
        use crate::updater::{BannerView, State};
        let state = self.updater.state().clone();
        let Some(BannerView { version, notes, text, error }) = state.banner() else { return };
        if state.should_quit() {
            // The new bundle is in place and started: hand over to it.
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        egui::Panel::top("update_banner").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("MacMischer {version} {text}")).strong().color(theme::ACCENT));
                let (label, update) = match &state {
                    State::Available { .. } => ("Open release page", false),
                    State::Updatable(_) => ("Update now", true),
                    State::Failed { .. } => ("Try again", true),
                    _ => ("", false),
                };
                if !label.is_empty() && ui.button(label).clicked() {
                    if update {
                        self.updater.update_now();
                    } else {
                        self.updater.open_release_page();
                    }
                }
                if state.is_dismissible() && ui.button("Later").clicked() {
                    self.updater.later();
                }
                if state.is_skippable() && ui.button("Skip this version").clicked() {
                    self.updater.skip_this_version();
                    Self::sync_update_prefs(&mut self.session.prefs, &self.updater);
                }
            });
            if let Some(e) = error {
                ui.label(egui::RichText::new(e).color(theme::DIM));
            }
            if !notes.trim().is_empty() {
                egui::ScrollArea::vertical().max_height(80.0).id_salt("update_notes").show(ui, |ui| {
                    ui.label(egui::RichText::new(notes.trim()).color(theme::DIM));
                });
            }
        });
    }
}

impl eframe::App for App {
    fn on_exit(&mut self) {
        self.session.save_now();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        {
            let m = &self.session.prefs.midi;
            self.midi.configure(m.enabled, m.device.as_deref(), self.midi_ui.open);
        }
        let midi_msgs = self.midi.drain();
        self.take_dropped_image(ui.ctx());
        self.background.sync(ui.ctx(), &self.session.prefs.background);
        self.background.paint(ui);
        self.update_banner(ui);
        if let Conn::Finding(f) = &mut self.conn {
            if let Some(addr) = f.ui(ui) {
                self.connect(addr);
            }
            return;
        }
        let ctx = ui.ctx().clone();
        let mut actions = Vec::new();
        self.keyboard(&ctx, &mut actions);
        let mut prefs_changed = self.midi_input(midi_msgs, &mut actions);
        // Tabs need &mut self; render them before borrowing mixer state.
        egui::Panel::bottom("banks").exact_size(40.0).show(ui, |ui| self.bank_tabs(ui, &mut actions));

        let mut switch = false;
        let locked = self.console.locked();
        let Conn::Mixer(mixer) = &self.conn else { unreachable!() };
        let status = mixer.status();
        let st_guard = mixer.state();
        let meters = mixer.meters().clone();
        let st = &*st_guard;
        let bank = self.session.banks.current();
        let rta_fresh = crate::rta::is_fresh(&meters);
        if !rta_fresh {
            self.rta_trace.clear();
        } else if !self.rta_ui.frozen {
            self.rta_trace.feed(&meters.rta, ctx.input(|i| i.stable_dt));
        }
        let mut c = views::Ctx {
            st,
            meters: &meters,
            ball: &mut self.ball,
            selected: self.console.selected(),
            actions: &mut actions,
            view: self.view,
            ab: self.console.ab_active(),
            presets: self.console.presets(),
            preset_undo: self.console.preset_undo_available(),
            rta_on: &mut self.rta_on,
            rta: &self.rta_trace,
            rta_shown: false,
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
                    let mode = if self.session.prefs.safe_mode { "Safe" } else { "Simple" };
                    let remote = match (self.session.remote.enabled, self.remote.clients()) {
                        (true, n) if n > 0 => format!(" · Remote: {n}"),
                        _ => String::new(),
                    };
                    if ui
                        .button(format!("Settings · {mode}{remote}"))
                        .on_hover_text("Web remote, safety, scroll wheel and updates (Ctrl/Cmd+,; Ctrl/Cmd+P opens the remote tab)")
                        .clicked()
                    {
                        c.actions.push(Action::OpenSettings);
                    }
                    ui.separator();
                    ui.label(egui::RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).color(theme::DIM))
                        .on_hover_text("MacMischer version");
                    ui.separator();
                    let mtxt = match (self.session.prefs.midi.enabled, self.midi.status()) {
                        (true, crate::midi::MidiStatus::Connected(_)) => egui::RichText::new("MIDI").color(theme::OK),
                        (true, _) => egui::RichText::new("MIDI").color(theme::WARN),
                        (false, _) => egui::RichText::new("MIDI"),
                    };
                    if ui.button(mtxt).on_hover_text("MIDI controller (Ctrl/Cmd+M)").clicked() {
                        c.actions.push(Action::OpenMidi);
                    }
                    if ui.button("RTA").on_hover_text("Spectrum analyzer window (Ctrl/Cmd+T)").clicked() {
                        c.actions.push(Action::OpenRta);
                    }
                    if ui.button("FX").on_hover_text("Effects rack (Ctrl/Cmd+F)").clicked() {
                        c.actions.push(Action::OpenFx);
                    }
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
        // Meter bridge: drag its bottom edge to resize; the height is remembered in the prefs.
        // Never let it grow so far that the detail panel and the strips (incl. the main strip) no longer fit.
        let bridge_max = (ui.available_height() - views::DETAIL_H - views::MIN_STRIPS_H)
            .clamp(views::BRIDGE_MIN_H, views::BRIDGE_MAX_H);
        let mut dock_out = crate::rta_view::Outcome::default();
        let mut dock_w = None;
        let docked = self.rta_ui.open && self.rta_ui.docked;
        let dock_default_w = self.session.prefs.ui.rta_dock_w.unwrap_or(420.0);
        let bridge_default = self.session.prefs.ui.bridge_h.unwrap_or(views::BRIDGE_H).min(bridge_max);
        let upper_h = self.upper_h.unwrap_or(bridge_default + views::DETAIL_H);
        let mut measured = None;
        let mut bridge_now = None;
        // One band across the top holds meters + detail on the left and, when docked, the RTA on the
        // right spanning both. Its height is what the two panels inside used last frame.
        egui::Panel::top("upper").exact_size(upper_h).frame(egui::Frame::NONE).show(ui, |ui| {
            if docked {
                let max_w = (ui.available_width() * 0.7).max(240.0);
                let r = egui::Panel::right("rta_dock")
                    .resizable(true)
                    .default_size(dock_default_w.clamp(240.0, max_w))
                    .size_range(240.0..=max_w)
                    .show(ui, |ui| crate::rta_view::contents(ui, &mut self.rta_ui, c.rta, c.st, rta_fresh, c.actions, 40.0));
                dock_out = r.inner;
                dock_w = Some(r.response.rect.width());
            }
            let bridge = egui::Panel::top("bridge")
                .resizable(true)
                .default_size(bridge_default)
                .size_range(views::BRIDGE_MIN_H..=bridge_max)
                .show(ui, |ui| views::meter_bridge(ui, &mut c));
            let detail = egui::Panel::top("detail").exact_size(views::DETAIL_H).show(ui, |ui| {
                ui.add_space(6.0);
                views::detail(ui, &mut c);
            });
            bridge_now = Some(bridge.response.rect.height());
            // The detail panel's own height is clamped by the space it gets, so it must not feed back;
            // the band is the bridge's height plus the detail panel's fixed height.
            let _ = detail;
            measured = Some(bridge.response.rect.height() + views::DETAIL_H);
        });
        if let Some(h) = measured {
            if (h - upper_h).abs() > 0.5 {
                ctx.request_repaint();
            }
            self.upper_h = Some(h);
        }
        if let Some(w) = dock_w {
            if crate::prefs::UiPrefs::moved(self.session.prefs.ui.rta_dock_w, w) {
                self.session.prefs.ui.rta_dock_w = Some(w);
                prefs_changed = true;
            }
        }
        if let Some(h) = bridge_now {
            if crate::prefs::UiPrefs::moved(self.session.prefs.ui.bridge_h, h) {
                self.session.prefs.ui.bridge_h = Some(h);
                prefs_changed = true;
            }
        }
        egui::Panel::right("main").exact_size(views::STRIP_W_STEREO + 16.0).show(ui, |ui| {
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
        // RTA subscription only while the EQ overlay is on and drawn, or the RTA window is open.
        mixer.set_rta_wanted((*c.rta_on && c.rta_shown) || self.rta_ui.open);
        let st_for_windows = st;
        if let Some(pl) = self.console.pending_load() {
            let mut answer = None;
            egui::Window::new("Load preset").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(&ctx, |ui| {
                ui.label(&pl.text);
                ui.horizontal(|ui| {
                    if ui.button("Load").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(false);
                    }
                });
            });
            match answer {
                Some(true) => actions.push(Action::PresetLoadConfirmed(pl.name.clone())),
                Some(false) => self.console.cancel_pending_load(),
                None => {}
            }
        }
        self.scenes.window(&ctx, st_for_windows, self.session.prefs.safe_mode, &mut actions);
        let win_out = crate::rta_view::rta_window(&ctx, &mut self.rta_ui, &self.rta_trace, st_for_windows, rta_fresh, &mut actions);
        if dock_out.toggle_dock || win_out.toggle_dock {
            self.rta_ui.docked = !self.rta_ui.docked;
            self.session.prefs.ui.rta_docked = self.rta_ui.docked;
            prefs_changed = true;
        }
        if dock_out.close {
            self.rta_ui.open = false;
        }
        crate::routing_view::routing_window(&ctx, &mut self.show_routing, st_for_windows, self.console.locked(), &mut actions);
        crate::fx_view::fx_window(&ctx, &mut self.show_fx, st_for_windows, self.console.locked(), &mut actions);
        {
            let (status, clients) = (self.remote.status(), self.remote.clients());
            let mut check = self.updater.prefs().check_at_launch;
            let changed = crate::settings_view::settings_window(
                &ctx,
                &mut self.settings,
                &mut self.session.prefs,
                &mut check,
                &mut self.session.remote,
                status,
                clients,
                self.session.banks.list(),
                self.session.banks.index(),
                &names,
                self.background.error.as_deref(),
            );
            if check != self.updater.prefs().check_at_launch {
                self.updater.set_check_at_launch(check);
                Self::sync_update_prefs(&mut self.session.prefs, &self.updater);
            }
            prefs_changed |= changed;
        }
        crate::widgets::set_scroll_disabled(self.session.prefs.disable_scroll);
        prefs_changed |= crate::midi_view::midi_window(
            &ctx,
            &mut self.midi_ui,
            &mut self.session.prefs.midi,
            &self.midi,
            &names,
            self.session.prefs.safe_mode,
        );
        drop(st_guard);

        if let Some(ed) = &mut self.editor {
            if let Some(result) = ed.show(&ctx, &names) {
                if let Some((i, bank)) = result {
                    self.session.banks.replace(i, bank);
                }
                self.editor = None;
            }
        }
        help_window(&ctx, &mut self.show_help);
        self.apply(actions);
        if switch {
            self.switch_mixer();
        }
        // Keep the web remote in sync with config and lock (cheap when unchanged).
        self.remote.configure(&self.session.remote);
        self.remote.set_locked(self.console.locked());
        if prefs_changed {
            self.session.prefs.save();
        }
        self.session.autosave(&ctx);
        self.background.outline_text(&ctx);
        // Keep meters animating smoothly between network packets.
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}
