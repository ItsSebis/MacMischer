//! MIDI control surface input: raw bytes -> [`MidiMsg`] -> bindings -> [`Effect`] -> [`Action`].
//!
//! Everything MIDI does goes through [`Action`] (and so through the lock filter and the undo
//! history). Mapping logic is pure and unit-tested; the device layer (`midir`, behind the `midi`
//! feature) only delivers parsed messages. Mixer writes are rate-limited by the network thread
//! (`Mixer::set` is coalesced, latest value wins), so a flood of CC messages is harmless.
//!
//! Conventions (same spirit as the web remote): values sent are absolute, faders have a level
//! cap (`max_db`, default +6 dB), and faders/pans use soft takeover so a controller never makes
//! the mixer jump.

use crate::actions::Action;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use xair_proto::{fader, Param, StripId, Value};

pub const DEFAULT_MAX_DB: f32 = 6.0;

// ---------------------------------------------------------------------------------------------
// Messages

/// A channel-voice message we care about. Channels are 1-16.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiMsg {
    Cc { ch: u8, cc: u8, value: u8 },
    NoteOn { ch: u8, note: u8, vel: u8 },
    NoteOff { ch: u8, note: u8 },
}

#[cfg_attr(not(feature = "midi"), allow(dead_code))]
impl MidiMsg {
    /// Parse one raw MIDI message. Sysex, realtime (clock, active sensing), program change, pitch
    /// bend etc. and malformed data return `None`. Note-on with velocity 0 is a note-off.
    pub fn parse(bytes: &[u8]) -> Option<MidiMsg> {
        let (&status, data) = bytes.split_first()?;
        if !(0x80..0xF0).contains(&status) {
            return None;
        }
        let ch = (status & 0x0F) + 1;
        let (&a, &b) = (data.first()?, data.get(1)?);
        if a >= 0x80 || b >= 0x80 {
            return None;
        }
        match status & 0xF0 {
            0x80 => Some(MidiMsg::NoteOff { ch, note: a }),
            0x90 if b == 0 => Some(MidiMsg::NoteOff { ch, note: a }),
            0x90 => Some(MidiMsg::NoteOn { ch, note: a, vel: b }),
            0xB0 => Some(MidiMsg::Cc { ch, cc: a, value: b }),
            _ => None,
        }
    }

    /// (channel, source) this message belongs to.
    pub fn key(&self) -> (u8, Source) {
        match *self {
            MidiMsg::Cc { ch, cc, .. } => (ch, Source::Cc(cc)),
            MidiMsg::NoteOn { ch, note, .. } | MidiMsg::NoteOff { ch, note } => (ch, Source::Note(note)),
        }
    }

    pub fn describe(&self) -> String {
        match *self {
            MidiMsg::Cc { ch, cc, value } => format!("ch {ch} CC {cc} = {value}"),
            MidiMsg::NoteOn { ch, note, vel } => format!("ch {ch} note {note} on ({vel})"),
            MidiMsg::NoteOff { ch, note } => format!("ch {ch} note {note} off"),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Bindings

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Source {
    Cc(u8),
    Note(u8),
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Cc(n) => write!(f, "cc:{n}"),
            Source::Note(n) => write!(f, "note:{n}"),
        }
    }
}

impl std::str::FromStr for Source {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let (kind, n) = s.split_once(':').ok_or_else(|| format!("bad MIDI source \"{s}\" (use cc:N or note:N)"))?;
        let n: u8 = n.trim().parse().map_err(|_| format!("bad number in \"{s}\""))?;
        if n > 127 {
            return Err(format!("{s}: number must be 0-127"));
        }
        match kind.trim() {
            "cc" => Ok(Source::Cc(n)),
            "note" => Ok(Source::Note(n)),
            _ => Err(format!("bad MIDI source \"{s}\" (use cc:N or note:N)")),
        }
    }
}

impl From<Source> for String {
    fn from(s: Source) -> String {
        s.to_string()
    }
}
impl TryFrom<String> for Source {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        s.parse()
    }
}

/// What a binding controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Target {
    /// Strip fader (CC only, with soft takeover).
    StripFader(StripId),
    /// Strip pan (CC only, with soft takeover).
    StripPan(StripId),
    /// Note-on toggles; CC sets (>= 64 = muted).
    StripMute(StripId),
    /// Note-on toggles; CC sets (>= 64 = soloed).
    StripSolo(StripId),
    /// Note-on / CC rising through 64 recalls the scene (refused in Safe mode).
    RecallScene(u8),
    /// Select the strip (note-on / CC rising through 64).
    Select(StripId),
}

impl Target {
    pub fn needs_cc(&self) -> bool {
        matches!(self, Target::StripFader(_) | Target::StripPan(_))
    }

    pub fn kind(&self) -> TargetKind {
        match self {
            Target::StripFader(_) => TargetKind::Fader,
            Target::StripPan(_) => TargetKind::Pan,
            Target::StripMute(_) => TargetKind::Mute,
            Target::StripSolo(_) => TargetKind::Solo,
            Target::RecallScene(_) => TargetKind::Scene,
            Target::Select(_) => TargetKind::Select,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match *self {
            Target::StripFader(s) | Target::StripMute(s) | Target::StripSolo(s) | Target::Select(s) => {
                s.is_valid().then_some(()).ok_or_else(|| format!("invalid strip {s:?}"))
            }
            Target::StripPan(s) if s.is_valid() && s.has_pan() => Ok(()),
            Target::StripPan(s) => Err(format!("strip {} has no pan", s.short())),
            Target::RecallScene(n) if (1..=crate::scenes::SLOTS).contains(&n) => Ok(()),
            Target::RecallScene(n) => Err(format!("scene {n} out of range 1-{}", crate::scenes::SLOTS)),
        }
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Target::StripFader(s) => write!(f, "fader:{}", s.short()),
            Target::StripPan(s) => write!(f, "pan:{}", s.short()),
            Target::StripMute(s) => write!(f, "mute:{}", s.short()),
            Target::StripSolo(s) => write!(f, "solo:{}", s.short()),
            Target::RecallScene(n) => write!(f, "scene:{n}"),
            Target::Select(s) => write!(f, "select:{}", s.short()),
        }
    }
}

impl std::str::FromStr for Target {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let (kind, arg) = s.split_once(':').ok_or_else(|| format!("bad target \"{s}\""))?;
        let strip = || StripId::from_short(arg).ok_or_else(|| format!("unknown strip \"{arg}\""));
        let t = match kind.trim() {
            "fader" => Target::StripFader(strip()?),
            "pan" => Target::StripPan(strip()?),
            "mute" => Target::StripMute(strip()?),
            "solo" => Target::StripSolo(strip()?),
            "select" => Target::Select(strip()?),
            "scene" => Target::RecallScene(arg.trim().parse().map_err(|_| format!("bad scene \"{arg}\""))?),
            _ => return Err(format!("unknown target kind \"{kind}\"")),
        };
        t.validate()?;
        Ok(t)
    }
}

impl From<Target> for String {
    fn from(t: Target) -> String {
        t.to_string()
    }
}
impl TryFrom<String> for Target {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        s.parse()
    }
}

/// Kinds of target, for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Fader,
    Pan,
    Mute,
    Solo,
    Scene,
    Select,
}

impl TargetKind {
    pub const ALL: [TargetKind; 6] =
        [TargetKind::Fader, TargetKind::Pan, TargetKind::Mute, TargetKind::Solo, TargetKind::Select, TargetKind::Scene];

    pub fn label(self) -> &'static str {
        match self {
            TargetKind::Fader => "Fader",
            TargetKind::Pan => "Pan",
            TargetKind::Mute => "Mute",
            TargetKind::Solo => "Solo",
            TargetKind::Scene => "Recall scene",
            TargetKind::Select => "Select strip",
        }
    }

    pub fn build(self, strip: StripId, scene: u8) -> Target {
        match self {
            TargetKind::Fader => Target::StripFader(strip),
            TargetKind::Pan => Target::StripPan(strip),
            TargetKind::Mute => Target::StripMute(strip),
            TargetKind::Solo => Target::StripSolo(strip),
            TargetKind::Scene => Target::RecallScene(scene),
            TargetKind::Select => Target::Select(strip),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    /// MIDI channel 1-16.
    pub channel: u8,
    pub source: Source,
    pub target: Target,
    /// Faders only: level cap in dB (fader law). `None` = no cap (+10 dB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_db: Option<f32>,
}

impl Binding {
    pub fn new(channel: u8, source: Source, target: Target) -> Binding {
        let max_db = matches!(target, Target::StripFader(_)).then_some(DEFAULT_MAX_DB);
        Binding { channel, source, target, max_db }
    }

    /// Faders always carry an explicit cap (a missing one gets the default; +10 = no cap).
    pub fn normalize(&mut self) {
        if matches!(self.target, Target::StripFader(_)) && self.max_db.is_none() {
            self.max_db = Some(DEFAULT_MAX_DB);
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(1..=16).contains(&self.channel) {
            return Err(format!("MIDI channel {} out of range 1-16", self.channel));
        }
        let (Source::Cc(n) | Source::Note(n)) = self.source;
        if n > 127 {
            return Err(format!("number {n} out of range 0-127"));
        }
        self.target.validate()?;
        if self.target.needs_cc() && !matches!(self.source, Source::Cc(_)) {
            return Err(format!("{} needs a CC controller, not a note", self.target));
        }
        if let Some(db) = self.max_db {
            if !(-90.0..=10.0).contains(&db) {
                return Err(format!("max_db {db} out of range -90..10"));
            }
        }
        Ok(())
    }

    fn key(&self) -> (u8, Source) {
        (self.channel, self.source)
    }

    pub fn source_label(&self) -> String {
        match self.source {
            Source::Cc(n) => format!("ch {} CC {n}", self.channel),
            Source::Note(n) => format!("ch {} note {n}", self.channel),
        }
    }
}

/// A set of bindings with unique (channel, source) pairs. Strict (de)serialisation for
/// export/import and tests; the preferences use the lenient [`sanitize`] path instead.
#[allow(dead_code)]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MidiMap {
    #[serde(default)]
    pub bindings: Vec<Binding>,
}

#[allow(dead_code)]
impl MidiMap {
    /// Check every binding and reject duplicate sources.
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = HashSet::new();
        for b in &self.bindings {
            b.validate()?;
            if !seen.insert(b.key()) {
                return Err(format!("duplicate binding for {}", b.source_label()));
            }
        }
        Ok(())
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|e| e.to_string())
    }

    /// Strict parse: any invalid or duplicate binding is an error.
    pub fn from_toml(text: &str) -> Result<MidiMap, String> {
        let mut m: MidiMap = toml::from_str(text).map_err(|e| e.to_string())?;
        m.bindings.iter_mut().for_each(Binding::normalize);
        m.validate()?;
        Ok(m)
    }
}

/// Keep valid bindings, drop invalid ones and later duplicates (logged). Used for prefs so that
/// one bad entry never discards the rest of the preferences file.
pub fn sanitize(bindings: impl IntoIterator<Item = Binding>) -> Vec<Binding> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for mut b in bindings {
        b.normalize();
        match b.validate() {
            Ok(()) if seen.insert(b.key()) => out.push(b),
            Ok(()) => tracing::warn!("ignoring duplicate MIDI binding for {}", b.source_label()),
            Err(e) => tracing::warn!("ignoring invalid MIDI binding: {e}"),
        }
    }
    out
}

/// MIDI settings stored in the preferences (not in the show file).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MidiPrefs {
    pub enabled: bool,
    /// Input port name.
    pub device: Option<String>,
    #[serde(deserialize_with = "lenient_bindings")]
    pub bindings: Vec<Binding>,
}

/// For `Prefs.midi`: a malformed `[midi]` table falls back to defaults instead of failing the
/// whole preferences file.
pub fn lenient_prefs<'de, D: serde::Deserializer<'de>>(d: D) -> Result<MidiPrefs, D::Error> {
    let v = toml::Value::deserialize(d)?;
    Ok(v.try_into::<MidiPrefs>().unwrap_or_else(|e| {
        tracing::warn!("ignoring unreadable [midi] preferences: {e}");
        MidiPrefs::default()
    }))
}

fn lenient_bindings<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Binding>, D::Error> {
    let vals: Vec<toml::Value> = Vec::deserialize(d)?;
    Ok(sanitize(vals.into_iter().filter_map(|v| {
        v.try_into::<Binding>()
            .inspect_err(|e| tracing::warn!("ignoring unreadable MIDI binding: {e}"))
            .ok()
    })))
}

// ---------------------------------------------------------------------------------------------
// Soft takeover

/// Tolerance for "controller is at the fader" (about two CC steps).
const PICKUP_TOL: f32 = 2.0 / 127.0;
/// If the mixer value moves this far from what we last sent, someone else moved it: re-arm.
const DRIFT: f32 = 0.02;
/// A "crossing" only counts if the controller moved this little between two messages ...
const CROSS_MAX_JUMP: f32 = 0.1;
/// ... or if the previous message is this recent (a fast sweep), never a stale one.
const CROSS_RECENT: Duration = Duration::from_millis(250);

/// Soft takeover (pickup): ignore the controller until it reaches or crosses the current value.
#[derive(Debug, Default, Clone)]
pub struct Pickup {
    engaged: bool,
    last_in: Option<f32>,
    last_at: Option<Instant>,
    last_out: Option<f32>,
}

impl Pickup {
    pub fn feed(&mut self, incoming: f32, current: f32) -> Option<f32> {
        self.feed_at(incoming, current, Instant::now())
    }

    /// `incoming` and `current` are positions 0..1. Returns the value to write, if engaged.
    pub fn feed_at(&mut self, incoming: f32, current: f32, now: Instant) -> Option<f32> {
        if self.engaged && self.last_out.is_some_and(|o| (current - o).abs() > DRIFT) {
            self.engaged = false;
        }
        if !self.engaged {
            let near = (incoming - current).abs() <= PICKUP_TOL;
            let crossed = self.last_in.is_some_and(|l| {
                (l - current) * (incoming - current) <= 0.0
                    && ((incoming - l).abs() <= CROSS_MAX_JUMP
                        || self.last_at.is_some_and(|t| now.saturating_duration_since(t) <= CROSS_RECENT))
            });
            self.engaged = near || crossed;
        }
        self.last_in = Some(incoming);
        self.last_at = Some(now);
        if self.engaged {
            self.last_out = Some(incoming);
            Some(incoming)
        } else {
            None
        }
    }
}

/// Largest fader position that does not exceed `db` (caps are floored, never rounded up).
fn cap_position(db: f32) -> f32 {
    (fader::from_db(db) * 1023.0).floor() / 1023.0
}

/// CC 0..127 -> fader position. Linear in fader position: 0 = -inf, 95 = -0.1 dB (unity is 0.75),
/// 127 = +10 dB; the binding's cap then limits the top end.
pub fn cc_to_fader(value: u8, max_db: Option<f32>) -> f32 {
    let pos = f32::from(value.min(127)) / 127.0;
    let cap = max_db.map_or(1.0, cap_position);
    fader::quantize(pos.min(cap))
}

/// CC 0..127 -> pan 0..1 with 64 exactly at the center.
pub fn cc_to_pan(value: u8) -> f32 {
    let v = value.min(127);
    if v <= 64 {
        f32::from(v) / 128.0
    } else {
        0.5 + f32::from(v - 64) / 126.0
    }
}

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

// ---------------------------------------------------------------------------------------------
// Learn

/// A pending "Learn": the next CC / note-on is bound to `target`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LearnReq {
    pub target: Target,
    /// Re-learn an existing row (keeps its cap) instead of adding one.
    pub replace: Option<usize>,
}

#[derive(Debug, PartialEq)]
pub enum LearnOutcome {
    /// Not usable for learning (note-off, note for a CC-only target): handle it normally.
    Ignored,
    Bound(usize),
    /// Source already bound elsewhere: message is consumed, keep waiting for another control.
    Taken(String),
}

pub fn learn(bindings: &mut Vec<Binding>, req: &LearnReq, msg: MidiMsg) -> LearnOutcome {
    let (channel, source) = match msg {
        MidiMsg::Cc { .. } => msg.key(),
        MidiMsg::NoteOn { .. } if !req.target.needs_cc() => msg.key(),
        _ => return LearnOutcome::Ignored,
    };
    if let Some(i) = bindings.iter().position(|b| b.key() == (channel, source)) {
        if Some(i) != req.replace {
            return LearnOutcome::Taken(format!(
                "{} is already bound to {} - move another control",
                bindings[i].source_label(),
                bindings[i].target
            ));
        }
    }
    match req.replace.filter(|&i| i < bindings.len()) {
        Some(i) => {
            bindings[i].channel = channel;
            bindings[i].source = source;
            LearnOutcome::Bound(i)
        }
        None => {
            bindings.push(Binding::new(channel, source, req.target));
            LearnOutcome::Bound(bindings.len() - 1)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Device layer

#[cfg_attr(not(feature = "midi"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
pub enum MidiStatus {
    Off,
    NoDeviceChosen,
    /// Chosen device is not plugged in; polling.
    Waiting(String),
    Connected(String),
    Error(String),
}

/// What the poller should do given what it sees.
#[cfg_attr(not(feature = "midi"), allow(dead_code))]
#[derive(Debug, PartialEq)]
pub enum Plan {
    Keep,
    Disconnect,
    Connect(String),
    Idle,
}

#[cfg_attr(not(feature = "midi"), allow(dead_code))]
pub fn plan(wanted: Option<&str>, ports: &[String], connected: Option<&str>) -> Plan {
    match (wanted, connected) {
        (None, Some(_)) => Plan::Disconnect,
        (None, None) => Plan::Idle,
        (Some(w), Some(c)) if w == c => {
            if ports.iter().any(|p| p == c) {
                Plan::Keep
            } else {
                Plan::Disconnect
            }
        }
        (Some(_), Some(_)) => Plan::Disconnect,
        (Some(w), None) if ports.iter().any(|p| p == w) => Plan::Connect(w.to_string()),
        (Some(_), None) => Plan::Idle,
    }
}

#[cfg(feature = "midi")]
mod device {
    use super::*;
    use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
    use std::sync::{Arc, Condvar, Mutex};

    const POLL: Duration = Duration::from_secs(2);

    #[derive(Default)]
    struct Shared {
        enabled: bool,
        watch: bool,
        device: Option<String>,
        ports: Vec<String>,
        status: Option<MidiStatus>,
        stop: bool,
    }

    type Sync_ = Arc<(Mutex<Shared>, Condvar)>;

    /// Lock that survives a poisoned mutex (a crashed worker must not take the UI down).
    fn lock(s: &Sync_) -> std::sync::MutexGuard<'_, Shared> {
        s.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// List input port names. Errors (no CoreMIDI/ALSA access, init failure) are returned, never panic.
    pub fn list_ports() -> Result<Vec<String>, String> {
        let input = midir::MidiInput::new("MacMischer list").map_err(|e| e.to_string())?;
        Ok(input.ports().iter().filter_map(|p| input.port_name(p).ok()).collect())
    }

    fn connect(name: &str, tx: SyncSender<(Instant, MidiMsg)>, ctx: eframe::egui::Context) -> Result<midir::MidiInputConnection<()>, String> {
        let input = midir::MidiInput::new("MacMischer").map_err(|e| e.to_string())?;
        let port = input
            .ports()
            .into_iter()
            .find(|p| input.port_name(p).is_ok_and(|n| n == name))
            .ok_or_else(|| "device disappeared".to_string())?;
        input
            .connect(
                &port,
                "MacMischer in",
                move |_, bytes, _| {
                    if let Some(m) = MidiMsg::parse(bytes) {
                        // Bounded: if the UI stalls, excess messages are dropped, not queued forever.
                        if tx.try_send((Instant::now(), m)).is_ok() {
                            ctx.request_repaint();
                        }
                    }
                },
                (),
            )
            .map_err(|e| e.to_string())
    }

    pub struct MidiService {
        shared: Sync_,
        rx: Receiver<(Instant, MidiMsg)>,
    }

    impl MidiService {
        pub fn new(ctx: eframe::egui::Context) -> MidiService {
            let shared: Sync_ = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
            let (tx, rx) = sync_channel(256);
            let sh = shared.clone();
            let spawned = std::thread::Builder::new().name("midi".into()).spawn(move || {
                let sh2 = sh.clone();
                let ctx2 = ctx.clone();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker(sh, tx, ctx)));
                if r.is_err() {
                    tracing::error!("MIDI thread panicked");
                    lock(&sh2).status = Some(MidiStatus::Error("MIDI thread crashed; restart the app".into()));
                    ctx2.request_repaint();
                }
            });
            if let Err(e) = spawned {
                tracing::warn!("cannot start MIDI thread: {e}");
                lock(&shared).status = Some(MidiStatus::Error(e.to_string()));
            }
            MidiService { shared, rx }
        }

        /// Cheap; wakes the poller only when something changed. `watch` = list ports even while disabled.
        pub fn configure(&self, enabled: bool, device: Option<&str>, watch: bool) {
            let cv = &self.shared.1;
            let mut s = lock(&self.shared);
            if s.enabled != enabled || s.watch != watch || s.device.as_deref() != device {
                s.enabled = enabled;
                s.watch = watch;
                s.device = device.map(str::to_string);
                cv.notify_all();
            }
        }

        pub fn drain(&self) -> Vec<MidiMsg> {
            fresh(self.rx.try_iter().collect(), Instant::now(), MAX_AGE)
        }

        pub fn ports(&self) -> Vec<String> {
            lock(&self.shared).ports.clone()
        }

        pub fn status(&self) -> MidiStatus {
            let s = lock(&self.shared);
            match &s.status {
                Some(st) => st.clone(),
                None => MidiStatus::Off,
            }
        }

        pub fn available() -> bool {
            true
        }
    }

    impl Drop for MidiService {
        fn drop(&mut self) {
            let (m, cv) = &*self.shared;
            if let Ok(mut s) = m.lock() {
                s.stop = true;
            }
            cv.notify_all();
        }
    }

    fn worker(shared: Sync_, tx: SyncSender<(Instant, MidiMsg)>, ctx: eframe::egui::Context) {
        let (m, cv) = &*shared;
        let mut conn: Option<(String, midir::MidiInputConnection<()>)> = None;
        loop {
            let (enabled, watch, device) = {
                let s = m.lock().unwrap_or_else(|e| e.into_inner());
                if s.stop {
                    return;
                }
                (s.enabled, s.watch, s.device.clone())
            };
            let mut status;
            let mut ports = Vec::new();
            if !enabled && !watch {
                conn = None;
                status = MidiStatus::Off;
            } else {
                match list_ports() {
                    Err(e) => {
                        conn = None;
                        status = MidiStatus::Error(e);
                    }
                    Ok(p) => {
                        ports = p;
                        let wanted = device.as_deref().filter(|_| enabled);
                        status = if enabled { MidiStatus::NoDeviceChosen } else { MidiStatus::Off };
                        // Two rounds: a disconnect may be followed by an immediate (re)connect.
                        for _ in 0..2 {
                            match plan(wanted, &ports, conn.as_ref().map(|(n, _)| n.as_str())) {
                                Plan::Keep | Plan::Idle => break,
                                Plan::Disconnect => conn = None,
                                Plan::Connect(name) => {
                                    match connect(&name, tx.clone(), ctx.clone()) {
                                        Ok(c) => conn = Some((name, c)),
                                        Err(e) => status = MidiStatus::Error(e),
                                    }
                                    break;
                                }
                            }
                        }
                        if enabled {
                            if let Some((n, _)) = &conn {
                                status = MidiStatus::Connected(n.clone());
                            } else if let Some(d) = wanted {
                                if !matches!(status, MidiStatus::Error(_)) {
                                    status = MidiStatus::Waiting(d.to_string());
                                }
                            }
                        }
                    }
                }
            }
            ctx.request_repaint();
            let mut s = m.lock().unwrap_or_else(|e| e.into_inner());
            s.ports = ports;
            s.status = Some(status);
            // Sleep until something changes or the next poll.
            if !s.stop {
                let (g, _) = cv.wait_timeout(s, POLL).unwrap_or_else(|e| e.into_inner());
                if g.stop {
                    return;
                }
            } else {
                return;
            }
        }
    }
}

#[cfg(not(feature = "midi"))]
mod device {
    use super::*;

    #[allow(dead_code)]
    pub fn list_ports() -> Result<Vec<String>, String> {
        Err("built without MIDI support".into())
    }

    pub struct MidiService;

    impl MidiService {
        pub fn new(_ctx: eframe::egui::Context) -> MidiService {
            MidiService
        }
        pub fn configure(&self, _enabled: bool, _device: Option<&str>, _watch: bool) {}
        pub fn drain(&self) -> Vec<MidiMsg> {
            Vec::new()
        }
        pub fn ports(&self) -> Vec<String> {
            Vec::new()
        }
        pub fn status(&self) -> MidiStatus {
            MidiStatus::Error("built without MIDI support (cargo feature `midi`)".into())
        }
        pub fn available() -> bool {
            false
        }
    }
}

pub use device::MidiService;
#[cfg(test)]
use device::list_ports;

// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        fader: f32,
        pan: f32,
        muted: Option<bool>,
        soloed: bool,
        partner: Option<StripId>,
    }
    impl View for Fake {
        fn fader(&self, _: StripId) -> Option<f32> {
            Some(self.fader)
        }
        fn pan(&self, _: StripId) -> Option<f32> {
            Some(self.pan)
        }
        fn muted(&self, _: StripId) -> Option<bool> {
            self.muted
        }
        fn soloed(&self, _: StripId) -> Option<bool> {
            Some(self.soloed)
        }
        fn scene_name(&self, n: u8) -> String {
            if n == 9 { String::new() } else { "Scene".into() }
        }
        fn partner(&self, _: StripId) -> Option<StripId> {
            self.partner
        }
    }
    fn fake() -> Fake {
        Fake { fader: 0.5, pan: 0.5, muted: Some(false), soloed: false, partner: None }
    }
    const CH1: StripId = StripId::Ch(1);
    fn cc(cc: u8, value: u8) -> MidiMsg {
        MidiMsg::Cc { ch: 1, cc, value }
    }

    #[test]
    fn parse_messages() {
        assert_eq!(MidiMsg::parse(&[0xB0, 7, 100]), Some(MidiMsg::Cc { ch: 1, cc: 7, value: 100 }));
        assert_eq!(MidiMsg::parse(&[0xBF, 1, 0]), Some(MidiMsg::Cc { ch: 16, cc: 1, value: 0 }));
        assert_eq!(MidiMsg::parse(&[0x92, 60, 90]), Some(MidiMsg::NoteOn { ch: 3, note: 60, vel: 90 }));
        assert_eq!(MidiMsg::parse(&[0x92, 60, 0]), Some(MidiMsg::NoteOff { ch: 3, note: 60 }));
        assert_eq!(MidiMsg::parse(&[0x82, 60, 40]), Some(MidiMsg::NoteOff { ch: 3, note: 60 }));
        // ignored / malformed
        assert_eq!(MidiMsg::parse(&[0xF0, 1, 2, 0xF7]), None);
        assert_eq!(MidiMsg::parse(&[0xF8]), None);
        assert_eq!(MidiMsg::parse(&[0xFE]), None);
        assert_eq!(MidiMsg::parse(&[0xC0, 5]), None);
        assert_eq!(MidiMsg::parse(&[0xE0, 0, 64]), None);
        assert_eq!(MidiMsg::parse(&[0xB0, 7]), None);
        assert_eq!(MidiMsg::parse(&[0xB0, 200, 1]), None);
        assert_eq!(MidiMsg::parse(&[0x40, 1, 1]), None);
        assert_eq!(MidiMsg::parse(&[]), None);
    }

    #[test]
    fn value_laws() {
        assert_eq!(cc_to_fader(0, None), 0.0);
        assert_eq!(cc_to_fader(127, None), 1.0);
        let db95 = fader::to_db(cc_to_fader(95, None));
        assert!(db95.abs() < 0.3, "{db95}");
        // cap
        assert!(fader::to_db(cc_to_fader(127, Some(0.0))).abs() < 0.1);
        assert!(fader::to_db(cc_to_fader(127, Some(6.0))) <= 6.05);
        assert_eq!(cc_to_pan(0), 0.0);
        assert_eq!(cc_to_pan(64), 0.5);
        assert_eq!(cc_to_pan(127), 1.0);
    }

    #[test]
    fn pickup_state_machine() {
        let mut p = Pickup::default();
        // far below the fader: ignored
        assert_eq!(p.feed(0.1, 0.5), None);
        assert_eq!(p.feed(0.3, 0.5), None);
        // crosses it: engages (and passes the value through)
        assert_eq!(p.feed(0.6, 0.5), Some(0.6));
        assert_eq!(p.feed(0.8, 0.6), Some(0.8));
        // someone moves the fader elsewhere: re-arm until crossed again
        assert_eq!(p.feed(0.81, 0.2), None);
        assert_eq!(p.feed(0.5, 0.2), None);
        assert_eq!(p.feed(0.1, 0.2), Some(0.1));
        // near on first message engages immediately
        let mut q = Pickup::default();
        assert_eq!(q.feed(0.51, 0.5), Some(0.51));
        // from above works too
        let mut r = Pickup::default();
        assert_eq!(r.feed(0.9, 0.5), None);
        assert_eq!(r.feed(0.4, 0.5), Some(0.4));
    }

    #[test]
    fn engine_fader_pickup_and_cap() {
        let b = [Binding::new(1, Source::Cc(7), Target::StripFader(CH1))];
        let mut e = Engine::default();
        let mut v = fake(); // fader 0.5
        assert!(e.handle(&b, MidiMsg::Cc { ch: 1, cc: 7, value: 0 }, &v, false).is_empty());
        assert!(e.handle(&b, MidiMsg::Cc { ch: 1, cc: 7, value: 30 }, &v, false).is_empty());
        let out = e.handle(&b, MidiMsg::Cc { ch: 1, cc: 7, value: 80 }, &v, false);
        assert_eq!(out, vec![Effect::Fader(CH1, cc_to_fader(80, Some(DEFAULT_MAX_DB)))]);
        v.fader = cc_to_fader(80, Some(DEFAULT_MAX_DB)); // the mixer now holds what we sent
        // wrong channel / other cc ignored
        assert!(e.handle(&b, MidiMsg::Cc { ch: 2, cc: 7, value: 80 }, &v, false).is_empty());
        assert!(e.handle(&b, MidiMsg::Cc { ch: 1, cc: 8, value: 80 }, &v, false).is_empty());
        // capped at +6 dB
        let top = e.handle(&b, MidiMsg::Cc { ch: 1, cc: 7, value: 127 }, &v, false);
        match top[..] {
            [Effect::Fader(_, f)] => assert!(fader::to_db(f) <= 6.05),
            _ => panic!("{top:?}"),
        }
    }

    #[test]
    fn engine_buttons() {
        let b = [
            Binding::new(1, Source::Note(36), Target::StripMute(CH1)),
            Binding::new(1, Source::Cc(20), Target::StripSolo(CH1)),
            Binding::new(1, Source::Note(37), Target::RecallScene(3)),
            Binding::new(1, Source::Cc(21), Target::Select(StripId::Bus(2))),
        ];
        let mut e = Engine::default();
        let v = fake();
        assert_eq!(e.handle(&b, MidiMsg::NoteOn { ch: 1, note: 36, vel: 100 }, &v, false), vec![Effect::Mute(CH1, Op::Toggle)]);
        assert!(e.handle(&b, MidiMsg::NoteOff { ch: 1, note: 36 }, &v, false).is_empty());
        assert_eq!(e.handle(&b, MidiMsg::Cc { ch: 1, cc: 20, value: 127 }, &v, false), vec![Effect::Solo(CH1, Op::Set(true))]);
        assert_eq!(e.handle(&b, MidiMsg::Cc { ch: 1, cc: 20, value: 0 }, &v, false), vec![Effect::Solo(CH1, Op::Set(false))]);
        assert_eq!(e.handle(&b, MidiMsg::NoteOn { ch: 1, note: 37, vel: 1 }, &v, false), vec![Effect::Scene(3)]);
        // CC buttons fire on the rising edge only
        let down = MidiMsg::Cc { ch: 1, cc: 21, value: 127 };
        assert_eq!(e.handle(&b, down, &v, false), vec![Effect::Select(StripId::Bus(2))]);
        assert!(e.handle(&b, down, &v, false).is_empty());
        assert!(e.handle(&b, MidiMsg::Cc { ch: 1, cc: 21, value: 0 }, &v, false).is_empty());
        assert_eq!(e.handle(&b, down, &v, false).len(), 1);
    }

    #[test]
    fn effects_to_actions() {
        let mut v = fake();
        assert_eq!(
            Effect::Mute(CH1, Op::Toggle).to_action(&v, true),
            Ok(Some(Action::Set(Param::On(CH1), Value::Int(0))))
        );
        v.muted = Some(true);
        assert_eq!(
            Effect::Mute(CH1, Op::Toggle).to_action(&v, true),
            Ok(Some(Action::Set(Param::On(CH1), Value::Int(1))))
        );
        assert_eq!(Effect::Mute(CH1, Op::Set(true)).to_action(&v, true), Ok(None));
        assert_eq!(Effect::Solo(CH1, Op::Toggle).to_action(&v, false), Ok(Some(Action::SetSolo(CH1, true))));
        // Safe mode refuses scene recall; empty scenes refused
        assert!(Effect::Scene(3).to_action(&v, true).is_err());
        assert_eq!(Effect::Scene(3).to_action(&v, false), Ok(Some(Action::RecallScene(3))));
        assert!(Effect::Scene(9).to_action(&v, false).is_err());
        // locked console filters faders via the normal Action filter
        let a = Effect::Fader(CH1, 0.5).to_action(&v, false).unwrap().unwrap();
        assert!(!a.allowed_when_locked(CH1));
        let a = Effect::Fader(StripId::Main, 0.5).to_action(&v, false).unwrap().unwrap();
        assert!(a.allowed_when_locked(CH1));
        assert!(!Effect::Scene(3).to_action(&v, false).unwrap().unwrap().allowed_when_locked(CH1));
    }

    #[test]
    fn toml_round_trip_and_validation() {
        let map = MidiMap {
            bindings: vec![
                Binding::new(1, Source::Cc(7), Target::StripFader(StripId::Bus(3))),
                Binding::new(2, Source::Note(60), Target::StripMute(StripId::Main)),
                Binding::new(16, Source::Cc(10), Target::StripPan(StripId::AuxIn)),
                Binding::new(1, Source::Note(61), Target::RecallScene(64)),
            ],
        };
        let text = map.to_toml().unwrap();
        assert!(text.contains("target = \"fader:B3\""), "{text}");
        assert_eq!(MidiMap::from_toml(&text).unwrap(), map);

        let bad = |t: &str| MidiMap::from_toml(&format!("[[bindings]]\n{t}\n")).is_err();
        assert!(bad("channel = 0\nsource = \"cc:1\"\ntarget = \"mute:1\""));
        assert!(bad("channel = 17\nsource = \"cc:1\"\ntarget = \"mute:1\""));
        assert!(bad("channel = 1\nsource = \"cc:128\"\ntarget = \"mute:1\""));
        assert!(bad("channel = 1\nsource = \"note:1\"\ntarget = \"fader:1\""));
        assert!(bad("channel = 1\nsource = \"cc:1\"\ntarget = \"pan:D1\""));
        assert!(bad("channel = 1\nsource = \"cc:1\"\ntarget = \"mute:99\""));
        assert!(bad("channel = 1\nsource = \"cc:1\"\ntarget = \"scene:65\""));
        assert!(bad("channel = 1\nsource = \"cc:1\"\ntarget = \"scene:0\""));
        assert!(bad("channel = 1\nsource = \"cc:1\"\ntarget = \"fader:1\"\nmax_db = 30.0"));
        assert!(bad("channel = 1\nsource = \"foo\"\ntarget = \"mute:1\""));
        let dup = "[[bindings]]\nchannel = 1\nsource = \"cc:1\"\ntarget = \"mute:1\"\n[[bindings]]\nchannel = 1\nsource = \"cc:1\"\ntarget = \"mute:2\"\n";
        assert!(MidiMap::from_toml(dup).unwrap_err().contains("duplicate"));
    }

    #[test]
    fn prefs_are_lenient() {
        let text = "enabled = true\ndevice = \"X\"\n\
            [[bindings]]\nchannel = 1\nsource = \"cc:1\"\ntarget = \"mute:1\"\n\
            [[bindings]]\nchannel = 99\nsource = \"cc:2\"\ntarget = \"mute:1\"\n\
            [[bindings]]\nchannel = 1\nsource = \"cc:1\"\ntarget = \"mute:2\"\n\
            [[bindings]]\ngarbage = 1\n";
        let p: MidiPrefs = toml::from_str(text).unwrap();
        assert!(p.enabled);
        assert_eq!(p.bindings.len(), 1);
        assert_eq!(toml::from_str::<MidiPrefs>("").unwrap(), MidiPrefs::default());
    }

    #[test]
    fn learning() {
        let mut b = vec![Binding::new(1, Source::Cc(7), Target::StripFader(CH1))];
        let req = LearnReq { target: Target::StripMute(CH1), replace: None };
        assert_eq!(learn(&mut b, &req, MidiMsg::NoteOff { ch: 1, note: 5 }), LearnOutcome::Ignored);
        assert_eq!(learn(&mut b, &req, MidiMsg::NoteOn { ch: 1, note: 5, vel: 9 }), LearnOutcome::Bound(1));
        assert_eq!(b[1].source, Source::Note(5));
        // already-bound source: consumed, keep waiting
        assert!(matches!(learn(&mut b, &req, MidiMsg::Cc { ch: 1, cc: 7, value: 1 }), LearnOutcome::Taken(_)));
        // CC-only target ignores notes
        let f = LearnReq { target: Target::StripFader(StripId::Ch(2)), replace: None };
        assert_eq!(learn(&mut b, &f, MidiMsg::NoteOn { ch: 1, note: 6, vel: 9 }), LearnOutcome::Ignored);
        // relearn keeps cap and may keep its own source
        let re = LearnReq { target: b[0].target, replace: Some(0) };
        assert_eq!(learn(&mut b, &re, MidiMsg::Cc { ch: 3, cc: 9, value: 1 }), LearnOutcome::Bound(0));
        assert_eq!((b[0].channel, b[0].source, b[0].max_db), (3, Source::Cc(9), Some(DEFAULT_MAX_DB)));
        assert_eq!(learn(&mut b, &re, MidiMsg::Cc { ch: 3, cc: 9, value: 1 }), LearnOutcome::Bound(0));
        assert!(b.iter().all(|x| x.validate().is_ok()));
    }

    #[test]
    fn reconnect_plan() {
        let ports = vec!["A".to_string(), "B".to_string()];
        assert_eq!(plan(None, &ports, None), Plan::Idle);
        assert_eq!(plan(None, &ports, Some("A")), Plan::Disconnect);
        assert_eq!(plan(Some("A"), &ports, None), Plan::Connect("A".into()));
        assert_eq!(plan(Some("A"), &ports, Some("A")), Plan::Keep);
        assert_eq!(plan(Some("C"), &ports, None), Plan::Idle);
        // device unplugged -> disconnect, then it is reconnected once it reappears
        assert_eq!(plan(Some("A"), &["B".to_string()], Some("A")), Plan::Disconnect);
        assert_eq!(plan(Some("A"), &ports, None), Plan::Connect("A".into()));
        // other device chosen
        assert_eq!(plan(Some("B"), &ports, Some("A")), Plan::Disconnect);
    }

    #[test]
    fn port_listing_never_panics() {
        // Without MIDI hardware / CoreMIDI access this is Ok(empty) or Err; either is fine.
        match list_ports() {
            Ok(p) => assert!(p.len() < 1000),
            Err(e) => assert!(!e.is_empty()),
        }
    }

    #[test]
    fn batch_sees_own_writes_with_unchanging_view() {
        // Fast sweep: many CCs in one frame while the mixer view never updates.
        let b = [Binding::new(1, Source::Cc(7), Target::StripFader(CH1))];
        let mut e = Engine::default();
        let v = fake(); // fader stays 0.5 in the base view
        let msgs: Vec<_> = [40, 60, 70, 75, 80, 85].iter().map(|&x| cc(7, x)).collect();
        let (acts, errs) = e.process(&b, &msgs, &v, false, false);
        assert!(errs.is_empty());
        // engages at the crossing (63 -> 70) and then keeps following every message
        let vals: Vec<f32> = acts
            .iter()
            .map(|a| match a {
                Action::Set(Param::Fader(_), Value::Float(f)) => *f,
                _ => panic!("{a:?}"),
            })
            .collect();
        assert_eq!(vals.len(), 4, "{vals:?}");
        assert!(vals.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn batch_toggle_twice_is_consistent() {
        let b = [Binding::new(1, Source::Note(36), Target::StripMute(CH1))];
        let mut e = Engine::default();
        let v = fake();
        let on = MidiMsg::NoteOn { ch: 1, note: 36, vel: 100 };
        let off = MidiMsg::NoteOff { ch: 1, note: 36 };
        let (acts, _) = e.process(&b, &[on, off, on], &v, false, false);
        assert_eq!(acts, vec![
            Action::Set(Param::On(CH1), Value::Int(0)),
            Action::Set(Param::On(CH1), Value::Int(1)),
        ]);
    }

    #[test]
    fn recall_stops_batch_and_levels_can_be_blocked() {
        let b = [
            Binding::new(1, Source::Note(37), Target::RecallScene(3)),
            Binding::new(1, Source::Cc(7), Target::StripFader(CH1)),
            Binding::new(1, Source::Note(36), Target::StripMute(CH1)),
        ];
        let mut e = Engine::default();
        let v = fake();
        let recall = MidiMsg::NoteOn { ch: 1, note: 37, vel: 1 };
        let mute = MidiMsg::NoteOn { ch: 1, note: 36, vel: 1 };
        let (acts, _) = e.process(&b, &[mute, recall, mute], &v, false, false);
        assert_eq!(acts.len(), 2);
        assert_eq!(acts[1], Action::RecallScene(3));
        // blocked levels: faders ignored, buttons still work
        let (acts, _) = e.process(&b, &[cc(7, 64), cc(7, 66), mute], &v, false, true);
        assert_eq!(acts.len(), 1);
        // unblocked: the same sweep moves the fader
        let (acts, _) = e.process(&b, &[cc(7, 64), cc(7, 66)], &v, false, false);
        assert!(!acts.is_empty());
    }

    #[test]
    fn pickup_ignores_stale_crossings() {
        let t0 = Instant::now();
        let mut p = Pickup::default();
        assert_eq!(p.feed_at(0.1, 0.5, t0), None);
        // a big jump after a long pause is not a crossing
        assert_eq!(p.feed_at(0.9, 0.5, t0 + Duration::from_secs(5)), None);
        // a big jump in a fast sweep is
        assert_eq!(p.feed_at(0.1, 0.5, t0 + Duration::from_secs(5) + Duration::from_millis(10)), Some(0.1));
        // a small step is a crossing even after a pause
        let mut q = Pickup::default();
        assert_eq!(q.feed_at(0.46, 0.5, t0), None);
        assert_eq!(q.feed_at(0.54, 0.5, t0 + Duration::from_secs(9)), Some(0.54));
    }

    #[test]
    fn learn_hold_and_stale_messages() {
        let t0 = Instant::now();
        let m = MidiMsg::NoteOn { ch: 1, note: 5, vel: 100 };
        let mut hold = Some(Hold { key: m.key(), at: t0 });
        assert!(hold_suppresses(&mut hold, m, t0 + Duration::from_millis(50)));
        assert!(!hold_suppresses(&mut hold, cc(1, 1), t0 + Duration::from_millis(60))); // other source
        assert!(hold_suppresses(&mut hold, MidiMsg::NoteOff { ch: 1, note: 5 }, t0 + Duration::from_millis(100)));
        assert!(hold.is_none()); // released
        assert!(!hold_suppresses(&mut hold, m, t0 + Duration::from_millis(120)));
        // CC buttons: release is value < 64; timeout also ends it
        let mut hold = Some(Hold { key: (1, Source::Cc(9)), at: t0 });
        assert!(hold_suppresses(&mut hold, cc(9, 127), t0 + Duration::from_millis(10)));
        assert!(hold_suppresses(&mut hold, cc(9, 0), t0 + Duration::from_millis(20)));
        assert!(hold.is_none());
        let mut hold = Some(Hold { key: (1, Source::Cc(9)), at: t0 });
        assert!(!hold_suppresses(&mut hold, cc(9, 127), t0 + Duration::from_millis(500)));
        assert!(hold.is_none());
        // stale queue entries are dropped
        let items = vec![(t0, cc(1, 1)), (t0 + Duration::from_millis(900), cc(2, 2))];
        assert_eq!(fresh(items, t0 + Duration::from_secs(1), MAX_AGE), vec![cc(2, 2)]);
    }

    #[test]
    fn fader_cap_defaults_and_flooring() {
        let text = "[[bindings]]\nchannel = 1\nsource = \"cc:1\"\ntarget = \"fader:1\"\n";
        assert_eq!(MidiMap::from_toml(text).unwrap().bindings[0].max_db, Some(DEFAULT_MAX_DB));
        let p: MidiPrefs = toml::from_str(text).unwrap();
        assert_eq!(p.bindings[0].max_db, Some(DEFAULT_MAX_DB));
        // explicit +10 means no cap
        let t10 = format!("{text}max_db = 10.0\n");
        assert_eq!(cc_to_fader(127, MidiMap::from_toml(&t10).unwrap().bindings[0].max_db), 1.0);
        // caps are floored: never above the requested dB
        for db in [-20.0f32, -7.3, 0.0, 2.2, 6.0] {
            assert!(fader::to_db(cc_to_fader(127, Some(db))) <= db + 1e-3, "{db}");
        }
    }

    #[test]
    fn linked_strips_use_lowest_cap() {
        let mut quiet = Binding::new(1, Source::Cc(8), Target::StripFader(StripId::Ch(2)));
        quiet.max_db = Some(-6.0);
        let b = [Binding::new(1, Source::Cc(7), Target::StripFader(CH1)), quiet];
        let mut e = Engine::default();
        let v = Fake { partner: Some(StripId::Ch(2)), ..fake() };
        let (acts, _) = e.process(&b, &[cc(7, 64), cc(7, 66), cc(7, 127)], &v, false, false);
        match acts.last() {
            Some(Action::Set(Param::Fader(_), Value::Float(f))) => assert!(fader::to_db(*f) <= -6.0 + 1e-3),
            other => panic!("{other:?}"),
        }
        // without a link the +6 dB cap applies
        let (acts, _) = Engine::default().process(&b, &[cc(7, 64), cc(7, 66), cc(7, 127)], &fake(), false, false);
        match acts.last() {
            Some(Action::Set(Param::Fader(_), Value::Float(f))) => assert!(fader::to_db(*f) > 0.0),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unsynced_mute_is_skipped() {
        let v = Fake { muted: None, ..fake() };
        assert_eq!(Effect::Mute(CH1, Op::Toggle).to_action(&v, false), Ok(None));
    }

    #[test]
    fn malformed_midi_table_does_not_reset_prefs() {
        let text = "safe_mode = true\nlast_mixer = \"1.2.3.4:10024\"\n[midi]\nenabled = \"yes\"\n";
        let p: crate::prefs::Prefs = toml::from_str(text).unwrap();
        assert!(p.safe_mode);
        assert_eq!(p.last_mixer.as_deref(), Some("1.2.3.4:10024"));
        assert_eq!(p.midi, MidiPrefs::default());
    }
}
