//! Mirrored mixer state + meters.

use std::collections::HashMap;
use std::time::Instant;
use xair_proto::{fader, meters, Param, SendTo, StripId, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MixerInfo {
    pub ip: String,
    pub name: String,
    pub model: String,
    pub firmware: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Status {
    /// Waiting for the mixer to answer `/xinfo`.
    Connecting,
    /// Reading the full state; `done` of `total` params received.
    Syncing { done: usize, total: usize },
    Connected,
    /// Was connected, mixer stopped answering; reconnecting.
    Lost,
}

/// All parameter values we know about. Missing = not yet synced.
#[derive(Debug, Default, Clone)]
pub struct MixerState {
    pub values: HashMap<Param, Value>,
    pub info: Option<MixerInfo>,
}

impl MixerState {
    pub fn get(&self, p: Param) -> Option<&Value> {
        self.values.get(&p)
    }
    fn f(&self, p: Param, default: f32) -> f32 {
        self.get(p).and_then(Value::as_f32).unwrap_or(default)
    }
    fn i(&self, p: Param, default: i32) -> i32 {
        self.get(p).and_then(Value::as_i32).unwrap_or(default)
    }

    /// Fader position 0..1.
    pub fn fader(&self, s: StripId) -> f32 {
        self.f(Param::Fader(s), 0.0)
    }
    pub fn fader_db(&self, s: StripId) -> f32 {
        fader::to_db(self.fader(s))
    }
    /// true = unmuted.
    pub fn on(&self, s: StripId) -> bool {
        self.i(Param::On(s), 1) != 0
    }
    pub fn pan(&self, s: StripId) -> f32 {
        self.f(Param::Pan(s), 0.5)
    }
    /// Mixer name, or a default label if empty.
    pub fn name(&self, s: StripId) -> String {
        match self.get(Param::Name(s)).and_then(Value::as_str) {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => s.to_string(),
        }
    }
    pub fn color(&self, s: StripId) -> i32 {
        self.i(Param::Color(s), 0)
    }
    /// Mute group membership bitmask (bit 0 = group 1).
    pub fn mute_mask(&self, s: StripId) -> i32 {
        self.i(Param::MuteGroups(s), 0)
    }
    pub fn mute_group_on(&self, n: u8) -> bool {
        self.i(Param::MuteGroupOn(n), 0) != 0
    }
    pub fn monitor_level(&self) -> f32 {
        self.f(Param::MonitorLevel, 0.0)
    }
    /// DCA membership bitmask (bit 0 = DCA 1).
    pub fn dca_mask(&self, s: StripId) -> i32 {
        self.i(Param::DcaGroups(s), 0)
    }
    pub fn send_level(&self, s: StripId, t: SendTo) -> f32 {
        self.f(Param::SendLevel(s, t), 0.0)
    }
    pub fn gain(&self, headamp: u8) -> f32 {
        self.f(Param::Gain(headamp), 0.0)
    }
    pub fn phantom(&self, headamp: u8) -> bool {
        self.i(Param::Phantom(headamp), 0) != 0
    }
    pub fn snap_index(&self) -> i32 {
        self.i(Param::SnapIndex, 0)
    }
    pub fn snap_name(&self, i: u8) -> String {
        self.get(Param::SnapName(i))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }
}

/// Latest meter values in dB.
#[derive(Debug, Clone)]
pub struct Meters {
    /// `/meters/1`: post-fader strip levels (see `xair_proto::meters::m1`).
    pub m1: Vec<f32>,
    /// `/meters/2`: 18 input (pre) + 18 USB return levels.
    pub m2: Vec<f32>,
    pub updated: Option<Instant>,
}

impl Default for Meters {
    fn default() -> Self {
        Meters {
            m1: vec![meters::FLOOR_DB; meters::m1::LEN],
            m2: vec![meters::FLOOR_DB; 36],
            updated: None,
        }
    }
}

impl Meters {
    /// Post-fader level for a strip; for stereo strips the louder side. `None` for DCAs.
    pub fn strip(&self, s: StripId) -> Option<f32> {
        use meters::m1::*;
        let at = |i: usize| self.m1.get(i).copied().unwrap_or(meters::FLOOR_DB);
        let pair = |i: usize| at(i).max(at(i + 1));
        Some(match s {
            StripId::Ch(n) => at(CH + n as usize - 1),
            StripId::AuxIn => pair(AUX),
            StripId::FxRtn(n) => pair(FX_RTN + 2 * (n as usize - 1)),
            StripId::Bus(n) => at(BUS + n as usize - 1),
            StripId::FxSend(n) => at(FX_SEND + n as usize - 1),
            StripId::Main => pair(MAIN),
            StripId::Dca(_) => return None,
        })
    }

    /// Stereo pair (L, R) for stereo strips.
    pub fn stereo(&self, s: StripId) -> Option<(f32, f32)> {
        use meters::m1::*;
        let i = match s {
            StripId::AuxIn => AUX,
            StripId::FxRtn(n) => FX_RTN + 2 * (n as usize - 1),
            StripId::Main => MAIN,
            _ => return None,
        };
        Some((*self.m1.get(i)?, *self.m1.get(i + 1)?))
    }
}
