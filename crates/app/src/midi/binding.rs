//! MIDI messages, bindings (what a control does) and their persistence.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use xair_proto::StripId;

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

    pub(super) fn key(&self) -> (u8, Source) {
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

