//! The open show: banks, file location, autosave, and the Show menu.

use crate::banks::BankSet;
use crate::prefs::Prefs;
use crate::show::{self, Show};
use crate::theme;
use eframe::egui;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const AUTOSAVE_AFTER: Duration = Duration::from_millis(800);

pub struct Session {
    pub banks: BankSet,
    pub name: String,
    pub path: PathBuf,
    pub prefs: Prefs,
    pub remote: xair_remote::RemoteConfig,
    saved: (BankSet, String, xair_remote::RemoteConfig),
    changed_at: Option<Instant>,
    new_name: String,
    rename: String,
    pub error: Option<String>,
}

enum Op {
    Open(PathBuf),
    New { from_current: bool },
    Import(PathBuf),
    Rename,
    RevealFolder,
}

impl Session {
    /// Load the last used show (or the default show) and the preferences.
    pub fn load() -> Session {
        let prefs = Prefs::load();
        let dir = show::shows_dir();
        let candidates = prefs.last_show.clone().into_iter().chain([show::path_for(&dir, "Default")]);
        let (show, path) = show::load_first(candidates)
            .unwrap_or_else(|| (Show::default_show(), show::path_for(&dir, "Default")));
        let mut s = Session::from_show(show, path, prefs);
        s.saved.1.clear(); // force first save so the file exists
        s.changed_at = Some(Instant::now() - AUTOSAVE_AFTER);
        s
    }

    fn from_show(show: Show, path: PathBuf, prefs: Prefs) -> Session {
        let banks = BankSet::new(show.banks, show.current_bank);
        Session {
            saved: (banks.clone(), show.name.clone(), show.remote.clone()),
            remote: show.remote,
            banks,
            rename: show.name.clone(),
            name: show.name,
            path,
            prefs,
            changed_at: None,
            new_name: String::new(),
            error: None,
        }
    }

    fn to_show(&self) -> Show {
        Show {
            name: self.name.clone(),
            banks: self.banks.list().to_vec(),
            current_bank: self.banks.index(),
            remote: self.remote.clone(),
        }
    }

    /// Write the show now if anything changed.
    pub fn save_now(&mut self) {
        let snapshot = (self.banks.clone(), self.name.clone(), self.remote.clone());
        if snapshot == self.saved {
            return;
        }
        match self.to_show().save(&self.path) {
            Ok(()) => {
                self.saved = snapshot;
                self.prefs.touch_show(&self.path);
                self.prefs.save();
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Saving show failed: {e}")),
        }
        self.changed_at = None;
    }

    /// Call once per frame: saves shortly after the last change.
    pub fn autosave(&mut self, ctx: &egui::Context) {
        let dirty = self.banks != self.saved.0 || self.name != self.saved.1 || self.remote != self.saved.2;
        if !dirty {
            self.changed_at = None;
            return;
        }
        let t = *self.changed_at.get_or_insert_with(Instant::now);
        if t.elapsed() >= AUTOSAVE_AFTER {
            self.save_now();
        } else {
            ctx.request_repaint_after(AUTOSAVE_AFTER);
        }
    }

    fn switch_to(&mut self, path: PathBuf, show: Show) {
        self.save_now();
        let prefs = std::mem::take(&mut self.prefs);
        *self = Session::from_show(show, path, prefs);
        self.prefs.touch_show(&self.path);
        self.prefs.save();
    }

    /// "Show ▾" menu (left of the bank tabs).
    pub fn menu(&mut self, ui: &mut egui::Ui) {
        let mut op = None;
        let title = egui::RichText::new(format!("Show: {}", self.name)).strong().color(theme::ACCENT);
        ui.menu_button(title, |ui| {
            ui.set_min_width(260.0);
            let dir = show::shows_dir();
            let shows = show::list_shows(&dir);
            ui.label(egui::RichText::new("OPEN").size(10.0).color(theme::DIM));
            for (name, path) in &shows {
                let current = *path == self.path;
                if ui.add_enabled(!current, egui::Button::new(name).selected(current)).clicked() {
                    op = Some(Op::Open(path.clone()));
                }
            }
            ui.separator();
            ui.label(egui::RichText::new("NEW SHOW").size(10.0).color(theme::DIM));
            ui.add(egui::TextEdit::singleline(&mut self.new_name).hint_text("name").desired_width(240.0));
            let ok = !self.new_name.trim().is_empty();
            ui.horizontal(|ui| {
                if ui.add_enabled(ok, egui::Button::new("Empty (default banks)")).clicked() {
                    op = Some(Op::New { from_current: false });
                }
                if ui.add_enabled(ok, egui::Button::new("Copy current banks")).clicked() {
                    op = Some(Op::New { from_current: true });
                }
            });
            ui.separator();
            ui.menu_button("Import banks from", |ui| {
                for (name, path) in &shows {
                    if *path != self.path && ui.button(name).clicked() {
                        op = Some(Op::Import(path.clone()));
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.rename).desired_width(150.0));
                if ui.button("Rename").clicked() {
                    op = Some(Op::Rename);
                }
            });
            if ui.button("Open shows folder").on_hover_text(dir.display().to_string()).clicked() {
                op = Some(Op::RevealFolder);
            }
        });
        if let Some(op) = op {
            ui.close();
            self.run(op);
        }
        if let Some(e) = &self.error {
            ui.colored_label(theme::MUTE, e);
        }
    }

    fn run(&mut self, op: Op) {
        let dir = show::shows_dir();
        match op {
            Op::Open(path) => match Show::load(&path) {
                Ok(s) => self.switch_to(path, s),
                Err(e) => self.error = Some(format!("Could not open show: {e}")),
            },
            Op::New { from_current } => {
                let name = self.new_name.trim().to_string();
                let path = show::unique_path(&dir, &name);
                let show = if from_current {
                    Show::from_template(&name, &self.to_show())
                } else {
                    Show { name: name.clone(), ..Show::default_show() }
                };
                self.new_name.clear();
                self.switch_to(path, show);
                self.saved.1.clear(); // write the new file right away
                self.save_now();
            }
            Op::Import(path) => match Show::load(&path) {
                Ok(other) => {
                    let mut cur = self.to_show();
                    cur.import_banks(&other);
                    self.banks = BankSet::new(cur.banks, self.banks.index());
                }
                Err(e) => self.error = Some(format!("Could not import: {e}")),
            },
            Op::Rename => {
                let name = self.rename.trim().to_string();
                if name.is_empty() || name == self.name {
                    return;
                }
                let new_path = show::unique_path(&dir, &name);
                self.name = name;
                if let Err(e) = show::move_file(&self.path, &new_path) {
                    self.error = Some(format!("Rename failed: {e}"));
                    return;
                }
                self.path = new_path;
                self.saved.1.clear();
                self.save_now();
            }
            Op::RevealFolder => {
                let _ = std::fs::create_dir_all(&dir);
                let opener = if cfg!(target_os = "macos") {
                    "open"
                } else if cfg!(target_os = "windows") {
                    "explorer"
                } else {
                    "xdg-open"
                };
                if let Err(e) = std::process::Command::new(opener).arg(&dir).spawn() {
                    self.error = Some(format!("Could not open folder: {e}"));
                }
            }
        }
    }
}
