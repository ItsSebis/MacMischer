//! The Console: applies operator [`Action`]s to the mixer under the Lock, Safe mode and undo
//! rules. It knows nothing about egui; the UI renders what it reports and feeds it Actions.
//!
//! Interface: [`Console::apply`] (Actions in, [`Outcome`] out) plus read-only accessors for the
//! state the UI draws. Actions that only toggle windows or move between banks (`SelectBank`,
//! `BankStep`, `Open*`, `SetView`) are handled by the app, not here.

use crate::actions::{nudge_fader, Action};
use crate::widgets::UNITY;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use xair_core::Mixer;
use xair_proto::{Param, SendTo, StripId, Value};

/// Fader/pan MIDI is ignored this long after a scene recall while the mixer re-reads its state.
const RESYNC: Duration = Duration::from_millis(600);

/// Read-only context the app owns and passes in on each call.
pub struct Env<'a> {
    /// The strips of the current bank, before linked pairs are folded into one.
    pub bank_strips: &'a [StripId],
    /// The bus of a bus-bound bank: the level controls then act on sends to it.
    pub level_bus: Option<SendTo>,
    pub safe_mode: bool,
    pub now: Instant,
}

/// What applying Actions produced for the operator.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// Message to show; the last one wins.
    pub flash: Option<String>,
    /// A scene was recalled (the MIDI soft-takeover must re-arm).
    pub recalled: bool,
}

/// A preset load waiting for the operator's confirmation.
pub struct PendingLoad {
    pub strip: StripId,
    pub name: String,
    pub text: String,
}

/// Values overwritten by the last preset load (strip and linked partner), for "Undo preset".
struct PresetUndo {
    strips: Vec<StripId>,
    old: crate::ab::Snapshot,
}

pub struct Console {
    selected: StripId,
    /// Lock: only navigation, mutes, solo and the main fader respond.
    locked: bool,
    history: crate::history::History,
    ab: crate::ab::Ab,
    preset_undo: Option<PresetUndo>,
    pending_load: Option<PendingLoad>,
    recall_at: Option<Instant>,
    presets: Vec<String>,
    preset_dir: PathBuf,
}

impl Console {
    pub fn new(preset_dir: PathBuf) -> Console {
        Console {
            selected: StripId::Ch(1),
            locked: false,
            history: Default::default(),
            ab: Default::default(),
            preset_undo: None,
            pending_load: None,
            recall_at: None,
            presets: crate::presets::list(&preset_dir),
            preset_dir,
        }
    }

    pub fn selected(&self) -> StripId {
        self.selected
    }

    /// Select a strip directly; a pending preset confirmation for another strip is dropped.
    pub fn select(&mut self, s: StripId) {
        self.selected = s;
        if self.pending_load.as_ref().is_some_and(|p| p.strip != s) {
            self.pending_load = None;
        }
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    pub fn pending_load(&self) -> Option<&PendingLoad> {
        self.pending_load.as_ref()
    }

    pub fn cancel_pending_load(&mut self) {
        self.pending_load = None;
    }

    pub fn presets(&self) -> &[String] {
        &self.presets
    }

    /// The A/B slot loaded for the selected strip, if A/B was started for it.
    pub fn ab_active(&self) -> Option<crate::ab::Slot> {
        self.ab.active(self.selected)
    }

    /// "Undo preset" is available for the selected strip.
    pub fn preset_undo_available(&self) -> bool {
        self.preset_undo.as_ref().is_some_and(|u| u.strips.contains(&self.selected))
    }

    /// Within the settling window after a scene recall.
    pub fn resyncing(&self, now: Instant) -> bool {
        self.recall_at.is_some_and(|t| now.duration_since(t) < RESYNC)
    }

    /// Forget undo history, A/B slots and preset undo (mixer switched).
    pub fn reset(&mut self) {
        self.history.clear();
        self.ab.clear();
        self.preset_undo = None;
    }

    pub fn apply(&mut self, m: &Mixer, env: &Env, actions: impl IntoIterator<Item = Action>) -> Outcome {
        let mut out = Outcome::default();
        for a in actions {
            self.apply_one(m, env, a, &mut out);
        }
        out
    }

    /// The level parameter the keyboard controls for strip `s` (send in a bus-bound bank).
    fn level_param(env: &Env, s: StripId) -> Param {
        match env.level_bus {
            Some(t) if s.has_sends() => Param::SendLevel(s, t),
            _ => Param::Fader(s),
        }
    }

    fn apply_one(&mut self, m: &Mixer, env: &Env, a: Action, out: &mut Outcome) {
        let s = self.selected;
        if self.locked && !a.allowed_when_locked(s) {
            out.flash = Some("Locked — Ctrl/Cmd+L to unlock".into());
            return;
        }
        match a {
            Action::Set(p, v) => {
                invalidate_preset_undo(&mut self.preset_undo, p);
                put(m, &mut self.history, p, v, env.now)
            }
            Action::Select(s) => {
                let leader = m.state().link_leader(s);
                self.select(leader);
            }
            Action::SelectPosition(n) => {
                let shown = m.state().display_strips(env.bank_strips);
                if let Some(s) = (n as usize).checked_sub(1).and_then(|i| shown.get(i)) {
                    self.select(*s);
                }
            }
            Action::StripStep(d) => {
                let strips = m.state().display_strips(env.bank_strips);
                if strips.is_empty() {
                    return;
                }
                let n = strips.len() as i32;
                let i = match strips.iter().position(|x| *x == s) {
                    Some(i) => (i as i32 + d).clamp(0, n - 1),
                    None if d < 0 => n - 1,
                    None => 0,
                };
                self.select(strips[i as usize]);
            }
            Action::Nudge(db) => {
                let p = Self::level_param(env, s);
                let cur = m.state().get(p).and_then(Value::as_f32).unwrap_or(0.0);
                put(m, &mut self.history, p, Value::Float(nudge_fader(cur, db)), env.now);
            }
            Action::Unity => {
                let p = Self::level_param(env, s);
                put(m, &mut self.history, p, Value::Float(UNITY), env.now);
            }
            Action::PanNudge(d) => {
                if s.has_pan() {
                    let v = (m.state().pan(s) + d).clamp(0.0, 1.0);
                    put(m, &mut self.history, Param::Pan(s), Value::Float(v), env.now);
                }
            }
            Action::ToggleMute => {
                let on = m.state().on(s);
                put(m, &mut self.history, Param::On(s), Value::Int(!on as i32), env.now);
            }
            Action::ToggleSolo => {
                let on = m.state().soloed(s);
                set_solo(m, s, !on);
            }
            Action::SetSolo(s, on) => set_solo(m, s, on),
            Action::RecallScene(i) => {
                self.recall_at = Some(env.now);
                self.reset();
                m.load_snapshot(i);
                out.recalled = true;
                out.flash = Some(format!("Recalled scene {i}"));
            }
            Action::SaveScene(i, name) => {
                m.save_snapshot(i);
                m.set(Param::SnapName(i), Value::Str(name));
                say(out, format!("Saved scene {i}"));
            }
            Action::RenameScene(i, name) => m.set(Param::SnapName(i), Value::Str(name)),
            Action::ToggleLock => self.locked = !self.locked,
            Action::Undo => match self.history.undo(|p| m.state().get(p).cloned()) {
                Some((p, v)) => {
                    m.set(p, v);
                    say(out, "Undo".into());
                }
                None => say(out, "Nothing to undo".into()),
            },
            Action::Redo => match self.history.redo(|p| m.state().get(p).cloned()) {
                Some((p, v)) => {
                    m.set(p, v);
                    say(out, "Redo".into());
                }
                None => say(out, "Nothing to redo".into()),
            },
            Action::AbToggle | Action::AbSelect(_) => {
                let params = xair_proto::param::proc_params(s);
                if params.is_empty() {
                    say(out, "No processing on this strip".into());
                    return;
                }
                let live: crate::ab::Snapshot = params
                    .iter()
                    .filter_map(|&p| {
                        let p = Param::Proc(s, p);
                        m.state().get(p).cloned().map(|v| (p, v))
                    })
                    .collect();
                if live.len() < params.len() {
                    say(out, "A/B: mixer still syncing".into());
                    return;
                }
                let writes = match a {
                    Action::AbSelect(slot) => self.ab.select(s, slot, live),
                    _ => self.ab.toggle(s, live),
                };
                // Not recorded in undo: the A/B slots already make a swap reversible, and
                // undoing single values would desync the active slot.
                for (p, v) in writes {
                    invalidate_preset_undo(&mut self.preset_undo, p);
                    m.set(p, v);
                }
            }
            Action::PresetSave(name) => {
                let st = m.state();
                let captured = crate::presets::capture(s, |p| st.get(p).cloned());
                drop(st);
                let msg = match captured {
                    None => "Preset: nothing to save (no processing, or mixer still syncing)".to_string(),
                    Some(pr) => match crate::presets::save(&self.preset_dir, &name, &pr) {
                        Ok(n) => format!("Saved preset \"{n}\""),
                        Err(e) => e,
                    },
                };
                self.presets = crate::presets::list(&self.preset_dir);
                say(out, msg);
            }
            Action::PresetLoad(name) => {
                if let Some(msg) = self.load_preset(m, s, &name, false, env.safe_mode) {
                    say(out, msg);
                }
            }
            Action::PresetLoadConfirmed(name) => {
                if let Some(msg) = self.load_preset(m, s, &name, true, false) {
                    say(out, msg);
                }
            }
            Action::PresetRefresh => self.presets = crate::presets::list(&self.preset_dir),
            Action::PresetDelete(name) => {
                let msg = match crate::presets::delete(&self.preset_dir, &name) {
                    Ok(()) => format!("Deleted preset \"{name}\""),
                    Err(e) => format!("Cannot delete preset: {e}"),
                };
                self.presets = crate::presets::list(&self.preset_dir);
                say(out, msg);
            }
            Action::PresetUndo => {
                let msg = match self.preset_undo.take() {
                    Some(u) => {
                        for (p, v) in u.old {
                            m.set(p, v);
                        }
                        "Undid preset load".to_string()
                    }
                    None => "No preset to undo".to_string(),
                };
                say(out, msg);
            }
            // Window and bank navigation belongs to the app.
            Action::SelectBank(_)
            | Action::BankStep(_)
            | Action::OpenScenes
            | Action::OpenRouting
            | Action::OpenFx
            | Action::OpenRemote
            | Action::OpenMidi
            | Action::SetView(_) => {}
        }
    }

    /// Load preset `name` onto `s`. Returns a flash message, or `None` when a confirmation
    /// prompt was queued instead.
    fn load_preset(&mut self, m: &Mixer, s: StripId, name: &str, confirmed: bool, safe: bool) -> Option<String> {
        let pr = match crate::presets::load(&self.preset_dir, name) {
            Ok(p) => p,
            Err(e) => return Some(e),
        };
        let pl = crate::presets::plan(&pr, s);
        if pl.writes.is_empty() {
            return Some("Preset has nothing for this strip".into());
        }
        let mismatch = crate::presets::needs_confirm(&pr, &pl, s);
        let target = {
            let st = m.state();
            format!("{} {}", s.short(), st.name(s)).trim().to_string()
        };
        if !confirmed && (mismatch || safe) {
            let mut text = format!("Load \"{name}\" onto {target}?");
            if mismatch {
                let kind = if pr.kind.is_empty() { "unknown" } else { pr.kind.as_str() };
                text += &format!("\nSaved from: {kind}; {} parameter(s) do not fit and will be skipped.", pl.skipped);
            }
            self.pending_load = Some(PendingLoad { strip: s, name: name.to_string(), text });
            return None;
        }
        self.pending_load = None;
        let st = m.state();
        // The linked partner follows the leader on the mixer; remember its old values too.
        let partner = st.linked_partner(s);
        let mut old: crate::ab::Snapshot = Vec::new();
        for x in std::iter::once(s).chain(partner) {
            for (p, _) in &pl.writes {
                if let Param::Proc(_, pr) = p {
                    let q = Param::Proc(x, *pr);
                    if let Some(v) = st.get(q).cloned() {
                        old.push((q, v));
                    }
                }
            }
        }
        drop(st);
        if old.len() < pl.writes.len() * (1 + usize::from(partner.is_some())) {
            return Some("Preset: mixer still syncing".into());
        }
        // Direct writes (not `put`): one "Undo preset" instead of ~50 history steps.
        for (p, v) in &pl.writes {
            m.set(*p, v.clone());
        }
        self.preset_undo = Some(PresetUndo { strips: std::iter::once(s).chain(partner).collect(), old });
        Some(format!("Loaded \"{name}\" onto {target}: {} applied, {} skipped", pl.writes.len(), pl.skipped))
    }
}

fn say(out: &mut Outcome, text: String) {
    out.flash = Some(text);
}

/// A later edit of a strip's processing (or a link change) makes the stored old values stale.
fn invalidate_preset_undo(undo: &mut Option<PresetUndo>, p: Param) {
    let stale = match p {
        Param::Proc(x, _) => undo.as_ref().is_some_and(|u| u.strips.contains(&x)),
        Param::ChLink(_) | Param::BusLink(_) => true,
        _ => false,
    };
    if stale {
        *undo = None;
    }
}

/// Set a parameter, remembering its previous value for undo.
fn put(m: &Mixer, h: &mut crate::history::History, p: Param, v: Value, now: Instant) {
    let old = m.state().get(p).cloned();
    h.record(p, old, now);
    m.set(p, v);
}

/// Solo a strip; linked partners follow (solo isn't mirrored by `Mixer::set`).
fn set_solo(m: &Mixer, s: StripId, on: bool) {
    let partner = m.state().linked_partner(s);
    for x in std::iter::once(s).chain(partner) {
        m.set(Param::Solo(x.solo_index()), Value::Int(on as i32));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xair_core::{Status, Timing};
    use xair_mock::MockMixer;
    use xair_proto::Proc;

    const CH1: StripId = StripId::Ch(1);

    struct Rig {
        _mock: MockMixer,
        mixer: Mixer,
        console: Console,
        dir: tempfile::TempDir,
        now: Instant,
        safe: bool,
    }

    impl Rig {
        fn new() -> Rig {
            let mock = MockMixer::start();
            let timing = Timing {
                lost_after: Duration::from_millis(500),
                hello_every: Duration::from_millis(100),
                ..Timing::default()
            };
            let mixer = Mixer::connect_with(mock.addr(), timing);
            let end = Instant::now() + Duration::from_secs(5);
            while mixer.status() != Status::Connected {
                assert!(Instant::now() < end, "mock mixer did not connect");
                std::thread::sleep(Duration::from_millis(10));
            }
            let dir = tempfile::tempdir().unwrap();
            let console = Console::new(dir.path().to_path_buf());
            Rig { _mock: mock, mixer, console, dir, now: Instant::now(), safe: false }
        }

        fn run(&mut self, actions: Vec<Action>) -> Outcome {
            let strips = [CH1, StripId::Ch(2), StripId::Ch(3)];
            let env = Env { bank_strips: &strips, level_bus: None, safe_mode: self.safe, now: self.now };
            self.console.apply(&self.mixer, &env, actions)
        }

        fn fader(&self, s: StripId) -> f32 {
            self.mixer.state().fader(s)
        }

        fn hpf(&self) -> f32 {
            self.mixer.state().get(Param::Proc(CH1, Proc::Hpf)).and_then(Value::as_f32).unwrap()
        }
    }

    #[test]
    fn lock_refuses_all_but_mutes_navigation_and_main() {
        let mut r = Rig::new();
        r.run(vec![Action::ToggleLock]);
        assert!(r.console.locked());
        let before = r.fader(CH1);
        let out = r.run(vec![Action::Nudge(3.0)]);
        assert_eq!(out.flash.as_deref(), Some("Locked — Ctrl/Cmd+L to unlock"));
        assert_eq!(r.fader(CH1), before);
        r.run(vec![Action::ToggleMute]);
        assert!(!r.mixer.state().on(CH1), "mute still works while locked");
        r.run(vec![Action::Select(StripId::Main), Action::Nudge(-3.0)]);
        assert_eq!(r.console.selected(), StripId::Main);
        assert!(r.fader(StripId::Main) < 0.75, "main fader works while locked");
        let out = r.run(vec![Action::RecallScene(1)]);
        assert!(!out.recalled && out.flash.is_some());
    }

    #[test]
    fn undo_restores_and_a_drag_is_one_step() {
        let mut r = Rig::new();
        let start = r.fader(CH1);
        r.run(vec![Action::Set(Param::Fader(CH1), Value::Float(0.3))]);
        r.run(vec![Action::Set(Param::Fader(CH1), Value::Float(0.4))]); // same instant: coalesced
        assert_eq!(r.fader(CH1), 0.4);
        let out = r.run(vec![Action::Undo]);
        assert_eq!(out.flash.as_deref(), Some("Undo"));
        assert_eq!(r.fader(CH1), start);
        r.run(vec![Action::Redo]);
        assert_eq!(r.fader(CH1), 0.4);
        assert_eq!(r.run(vec![Action::Redo]).flash.as_deref(), Some("Nothing to redo"));
    }

    #[test]
    fn scene_recall_clears_history_and_opens_the_resync_window() {
        let mut r = Rig::new();
        r.run(vec![Action::Set(Param::Fader(CH1), Value::Float(0.3))]);
        assert!(!r.console.resyncing(r.now));
        let out = r.run(vec![Action::RecallScene(1)]);
        assert!(out.recalled);
        assert!(r.console.resyncing(r.now + Duration::from_millis(100)));
        assert!(!r.console.resyncing(r.now + Duration::from_millis(700)));
        assert_eq!(r.run(vec![Action::Undo]).flash.as_deref(), Some("Nothing to undo"));
    }

    #[test]
    fn preset_save_load_and_undo() {
        let mut r = Rig::new();
        let original = r.hpf();
        let out = r.run(vec![Action::PresetSave("t".into())]);
        assert_eq!(out.flash.as_deref(), Some("Saved preset \"t\""));
        assert_eq!(r.console.presets(), ["t".to_string()]);
        assert!(r.dir.path().read_dir().unwrap().count() > 0, "written into the injected dir");
        let p = Param::Proc(CH1, Proc::Hpf);
        r.run(vec![Action::Set(p, Value::Float(0.9))]);
        assert!((r.hpf() - 0.9).abs() < 1e-6 && (original - 0.9).abs() > 0.05);
        let out = r.run(vec![Action::PresetLoad("t".into())]);
        assert!(out.flash.unwrap().starts_with("Loaded \"t\""));
        assert!((r.hpf() - original).abs() < 0.02);
        assert!(r.console.preset_undo_available());
        r.run(vec![Action::PresetUndo]);
        assert!((r.hpf() - 0.9).abs() < 1e-6);
        assert!(!r.console.preset_undo_available());
    }

    #[test]
    fn safe_mode_asks_before_loading_and_selecting_elsewhere_drops_the_prompt() {
        let mut r = Rig::new();
        r.run(vec![Action::PresetSave("t".into())]);
        let p = Param::Proc(CH1, Proc::Hpf);
        r.run(vec![Action::Set(p, Value::Float(0.9))]);
        r.safe = true;
        let out = r.run(vec![Action::PresetLoad("t".into())]);
        assert_eq!(out.flash, None);
        assert_eq!(r.console.pending_load().map(|p| p.name.as_str()), Some("t"));
        assert!((r.hpf() - 0.9).abs() < 1e-6, "nothing written before confirming");
        r.run(vec![Action::Select(StripId::Ch(2))]);
        assert!(r.console.pending_load().is_none());
        r.run(vec![Action::Select(CH1), Action::PresetLoad("t".into()), Action::PresetLoadConfirmed("t".into())]);
        assert!(r.console.pending_load().is_none());
        assert!((r.hpf() - 0.9).abs() > 0.05, "confirmed load applied");
    }

    #[test]
    fn ab_swaps_processing_without_touching_undo() {
        let mut r = Rig::new();
        let p = Param::Proc(CH1, Proc::Hpf);
        let a = r.hpf();
        assert_eq!(r.console.ab_active(), None);
        r.run(vec![Action::AbToggle]); // starts A/B on slot A, no writes
        assert_eq!(r.console.ab_active(), Some(crate::ab::Slot::A));
        r.run(vec![Action::Set(p, Value::Float(0.9))]);
        r.run(vec![Action::AbToggle]); // to B: B was captured equal to A's original
        assert_eq!(r.console.ab_active(), Some(crate::ab::Slot::B));
        assert!((r.hpf() - a).abs() < 1e-6);
        r.run(vec![Action::AbToggle]); // back to A: A got the edited value saved
        assert!((r.hpf() - 0.9).abs() < 1e-6);
    }
}
