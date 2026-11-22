//! Messages -> effects -> Actions, with the mixer view they need.

use super::*;
use crate::actions::Action;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use xair_proto::{fader, Param, StripId, Value};

// ---------------------------------------------------------------------------------------------
// Engine: messages -> effects

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Toggle,
    /// `true` = muted / soloed.
    Set(bool),
}

/// What a message wants done, before looking at mixer state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Effect {
    Fader(StripId, f32),
    Pan(StripId, f32),
    Mute(StripId, Op),
    Solo(StripId, Op),
    Scene(u8),
    Select(StripId),
}

/// Read access to the mixer for pickup and state-dependent effects. `None` = not synced yet.
pub trait View {
    fn fader(&self, s: StripId) -> Option<f32>;
    fn pan(&self, s: StripId) -> Option<f32>;
    fn muted(&self, s: StripId) -> Option<bool>;
    fn soloed(&self, s: StripId) -> Option<bool>;
    fn scene_name(&self, n: u8) -> String;
    /// Stereo-linked partner (the mixer mirrors writes to it).
    fn partner(&self, s: StripId) -> Option<StripId>;
}

/// Mixer state plus the writes already emitted earlier in the same batch of messages, so that
/// several messages handled before the next frame do not see stale values.
pub struct Overlay<'a> {
    base: &'a dyn View,
    fader: HashMap<StripId, f32>,
    pan: HashMap<StripId, f32>,
    muted: HashMap<StripId, bool>,
    soloed: HashMap<StripId, bool>,
}

impl<'a> Overlay<'a> {
    pub fn new(base: &'a dyn View) -> Overlay<'a> {
        Overlay { base, fader: HashMap::new(), pan: HashMap::new(), muted: HashMap::new(), soloed: HashMap::new() }
    }

    /// Remember what an emitted action will write (including the linked partner).
    pub fn record(&mut self, a: &Action) {
        let targets = |s: StripId| std::iter::once(s).chain(self.base.partner(s)).collect::<Vec<_>>();
        match a {
            Action::Set(Param::Fader(s), Value::Float(v)) => {
                for x in targets(*s) {
                    self.fader.insert(x, *v);
                }
            }
            Action::Set(Param::Pan(s), Value::Float(v)) => {
                for x in targets(*s) {
                    self.pan.insert(x, *v);
                }
            }
            Action::Set(Param::On(s), Value::Int(i)) => {
                for x in targets(*s) {
                    self.muted.insert(x, *i == 0);
                }
            }
            Action::SetSolo(s, on) => {
                for x in targets(*s) {
                    self.soloed.insert(x, *on);
                }
            }
            _ => {}
        }
    }
}

impl View for Overlay<'_> {
    fn fader(&self, s: StripId) -> Option<f32> {
        self.fader.get(&s).copied().or_else(|| self.base.fader(s))
    }
    fn pan(&self, s: StripId) -> Option<f32> {
        self.pan.get(&s).copied().or_else(|| self.base.pan(s))
    }
    fn muted(&self, s: StripId) -> Option<bool> {
        self.muted.get(&s).copied().or_else(|| self.base.muted(s))
    }
    fn soloed(&self, s: StripId) -> Option<bool> {
        self.soloed.get(&s).copied().or_else(|| self.base.soloed(s))
    }
    fn scene_name(&self, n: u8) -> String {
        self.base.scene_name(n)
    }
    fn partner(&self, s: StripId) -> Option<StripId> {
        self.base.partner(s)
    }
}

/// Lowest cap among the fader bindings of `s` and its linked partner (the write is mirrored).
fn effective_cap(bindings: &[Binding], own: Option<f32>, s: StripId, partner: Option<StripId>) -> Option<f32> {
    bindings
        .iter()
        .filter_map(|x| match x.target {
            Target::StripFader(t) if t == s || Some(t) == partner => x.max_db,
            _ => None,
        })
        .fold(own, |a, b| Some(a.map_or(b, |a| a.min(b))))
}

/// Per-source runtime state (pickup, button edges).
#[derive(Default)]
pub struct Engine {
    pickups: HashMap<(u8, Source), Pickup>,
    held: HashSet<(u8, Source)>,
}

impl Engine {
    /// Map a whole batch of messages to actions, in order. Writes emitted earlier in the batch are
    /// visible to later messages. Processing stops after a scene recall (everything after it
    /// would act on the old scene). `block_levels` drops fader/pan messages (mixer re-reading).
    /// Returns the actions and user-facing refusal messages.
    pub fn process(
        &mut self,
        bindings: &[Binding],
        msgs: &[MidiMsg],
        base: &dyn View,
        safe_mode: bool,
        block_levels: bool,
    ) -> (Vec<Action>, Vec<String>) {
        let mut ov = Overlay::new(base);
        let (mut actions, mut errors) = (Vec::new(), Vec::new());
        for &msg in msgs {
            for e in self.handle(bindings, msg, &ov, block_levels) {
                match e.to_action(&ov, safe_mode) {
                    Ok(Some(a)) => {
                        ov.record(&a);
                        let recall = matches!(a, Action::RecallScene(_));
                        actions.push(a);
                        if recall {
                            self.reset();
                            return (actions, errors);
                        }
                    }
                    Ok(None) => {}
                    Err(t) => errors.push(t),
                }
            }
        }
        (actions, errors)
    }

    pub fn handle(&mut self, bindings: &[Binding], msg: MidiMsg, view: &dyn View, block_levels: bool) -> Vec<Effect> {
        let key = msg.key();
        // Edge detection for "button" sources, shared by all targets of this source.
        let pressed = match msg {
            MidiMsg::NoteOn { .. } => true,
            MidiMsg::Cc { value, .. } if value >= 64 => !self.held.contains(&key),
            _ => false,
        };
        let mut out = Vec::new();
        for b in bindings.iter().filter(|b| b.key() == key) {
            match (b.target, msg) {
                (Target::StripFader(_) | Target::StripPan(_), _) if block_levels => {}
                (Target::StripFader(s), MidiMsg::Cc { value, .. }) => {
                    let Some(cur) = view.fader(s) else { continue };
                    let cap = effective_cap(bindings, b.max_db, s, view.partner(s));
                    let p = self.pickups.entry(key).or_default();
                    if let Some(v) = p.feed(cc_to_fader(value, cap), cur) {
                        if fader::quantize(cur) != v {
                            out.push(Effect::Fader(s, v));
                        }
                    }
                }
                (Target::StripPan(s), MidiMsg::Cc { value, .. }) => {
                    let Some(cur) = view.pan(s) else { continue };
                    let p = self.pickups.entry(key).or_default();
                    if let Some(v) = p.feed(cc_to_pan(value), cur) {
                        if fader::quantize(cur) != fader::quantize(v) {
                            out.push(Effect::Pan(s, fader::quantize(v)));
                        }
                    }
                }
                (Target::StripMute(s), MidiMsg::Cc { value, .. }) => out.push(Effect::Mute(s, Op::Set(value >= 64))),
                (Target::StripMute(s), MidiMsg::NoteOn { .. }) => out.push(Effect::Mute(s, Op::Toggle)),
                (Target::StripSolo(s), MidiMsg::Cc { value, .. }) => out.push(Effect::Solo(s, Op::Set(value >= 64))),
                (Target::StripSolo(s), MidiMsg::NoteOn { .. }) => out.push(Effect::Solo(s, Op::Toggle)),
                (Target::RecallScene(n), _) if pressed => out.push(Effect::Scene(n)),
                (Target::Select(s), _) if pressed => out.push(Effect::Select(s)),
                _ => {}
            }
        }
        match msg {
            MidiMsg::NoteOn { .. } => {
                self.held.insert(key);
            }
            MidiMsg::Cc { value, .. } if value >= 64 => {
                self.held.insert(key);
            }
            _ => {
                self.held.remove(&key);
            }
        }
        out
    }

    /// Forget pickup state (mixer/device/bindings changed, scene recalled).
    pub fn reset(&mut self) {
        self.pickups.clear();
        self.held.clear();
    }
}

impl Effect {
    /// Turn an effect into an [`Action`] given the current mixer state. `Ok(None)` = nothing to
    /// do (or state not synced); `Err` = refused, with a message for the user. Scene recall is
    /// refused in Safe mode because it cannot be confirmed interactively.
    pub fn to_action(self, view: &dyn View, safe_mode: bool) -> Result<Option<Action>, String> {
        Ok(match self {
            Effect::Fader(s, v) => Some(Action::Set(Param::Fader(s), Value::Float(v))),
            Effect::Pan(s, v) => Some(Action::Set(Param::Pan(s), Value::Float(v))),
            Effect::Mute(s, op) => {
                let Some(now) = view.muted(s) else { return Ok(None) };
                let want = match op {
                    Op::Toggle => !now,
                    Op::Set(b) => b,
                };
                (want != now).then(|| Action::Set(Param::On(s), Value::Int(i32::from(!want))))
            }
            Effect::Solo(s, op) => {
                let Some(now) = view.soloed(s) else { return Ok(None) };
                let want = match op {
                    Op::Toggle => !now,
                    Op::Set(b) => b,
                };
                (want != now).then_some(Action::SetSolo(s, want))
            }
            Effect::Select(s) => Some(Action::Select(s)),
            Effect::Scene(n) => {
                if safe_mode {
                    return Err(format!("MIDI scene recall refused: Safe mode is on (scene {n})"));
                }
                if view.scene_name(n).is_empty() {
                    return Err(format!("MIDI: scene {n} is empty"));
                }
                Some(Action::RecallScene(n))
            }
        })
    }
}

/// Drop messages that sat in the queue too long (UI stalled): acting on them would be a surprise.
#[cfg_attr(not(feature = "midi"), allow(dead_code))]
pub fn fresh(items: Vec<(Instant, MidiMsg)>, now: Instant, max_age: Duration) -> Vec<MidiMsg> {
    items.into_iter().filter(|(t, _)| now.saturating_duration_since(*t) <= max_age).map(|(_, m)| m).collect()
}

#[cfg_attr(not(feature = "midi"), allow(dead_code))]
pub const MAX_AGE: Duration = Duration::from_millis(400);
const HOLD_FOR: Duration = Duration::from_millis(300);

/// After Learn binds a control, that control is ignored until it is released (note-off, CC < 64)
/// or [`HOLD_FOR`] passes, so the learning gesture itself does not trigger the new binding.
#[derive(Debug, Clone, Copy)]
pub struct Hold {
    pub key: (u8, Source),
    pub at: Instant,
}

pub fn hold_suppresses(hold: &mut Option<Hold>, msg: MidiMsg, now: Instant) -> bool {
    let Some(h) = *hold else { return false };
    if now.saturating_duration_since(h.at) > HOLD_FOR {
        *hold = None;
        return false;
    }
    if msg.key() != h.key {
        return false;
    }
    if matches!(msg, MidiMsg::NoteOff { .. } | MidiMsg::Cc { value: 0..=63, .. }) {
        *hold = None;
    }
    true
}

