//! Mirrored mixer state + meters.

use std::collections::HashMap;
use std::time::Instant;
use xair_proto::{fader, meters, Param, Proc, SendTo, StripId, Value};

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
    pub fn soloed(&self, s: StripId) -> bool {
        self.i(Param::Solo(s.solo_index()), 0) != 0
    }

    /// The stereo-linked partner of a channel or bus, if linked.
    pub fn linked_partner(&self, s: StripId) -> Option<StripId> {
        let (n, link, mk): (u8, Param, fn(u8) -> StripId) = match s {
            StripId::Ch(n) => (n, Param::ChLink(n.div_ceil(2)), StripId::Ch),
            StripId::Bus(n) => (n, Param::BusLink(n.div_ceil(2)), StripId::Bus),
            _ => return None,
        };
        if self.i(link, 0) == 0 {
            return None;
        }
        Some(mk(if n % 2 == 1 { n + 1 } else { n - 1 }))
    }

    /// Odd (left) strip of a linked pair, else the strip itself.
    pub fn link_leader(&self, s: StripId) -> StripId {
        match self.linked_partner(s) {
            Some(p) if p < s => p,
            _ => s,
        }
    }

    /// Bank strips as displayed: linked pairs collapse onto their leader, order kept.
    pub fn display_strips(&self, strips: &[StripId]) -> Vec<StripId> {
        let mut out: Vec<StripId> = Vec::with_capacity(strips.len());
        for &s in strips {
            let l = self.link_leader(s);
            if !out.contains(&l) {
                out.push(l);
            }
        }
        out
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
    /// Raw 0..1 float of a processing parameter (0 if unknown).
    pub fn proc_f(&self, s: StripId, p: Proc) -> f32 {
        self.f(Param::Proc(s, p), 0.0)
    }
    /// Integer value of a processing switch / enum (0 if unknown).
    pub fn proc_i(&self, s: StripId, p: Proc) -> i32 {
        self.i(Param::Proc(s, p), 0)
    }
    /// Real-world value (Hz, dB, ms, ...) of a scaled float processing parameter.
    pub fn proc_value(&self, s: StripId, p: Proc) -> Option<f32> {
        Some(p.scale()?.to_value(self.proc_f(s, p)))
    }
    pub fn fx_type(&self, slot: u8) -> i32 {
        self.i(Param::FxType(slot), 0)
    }
    pub fn fx_par(&self, slot: u8, par: u8) -> f32 {
        self.f(Param::FxPar(slot, par), 0.0)
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
    /// `/meters/4` RTA spectrum (100 dB values, UNVERIFIED layout); empty unless the RTA is
    /// wanted (see `Mixer::set_rta_wanted`) and a blob has arrived.
    pub rta: Vec<f32>,
    pub rta_updated: Option<Instant>,
}

impl Default for Meters {
    fn default() -> Self {
        Meters {
            m1: vec![meters::FLOOR_DB; meters::m1::LEN],
            m2: vec![meters::FLOOR_DB; 36],
            updated: None,
            rta: Vec::new(),
            rta_updated: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use xair_proto::meters::m1;

    fn linked(ch_pairs: &[u8], bus_pairs: &[u8]) -> MixerState {
        let mut st = MixerState::default();
        for &p in ch_pairs {
            st.values.insert(Param::ChLink(p), Value::Int(1));
        }
        for &p in bus_pairs {
            st.values.insert(Param::BusLink(p), Value::Int(1));
        }
        st
    }

    #[test]
    fn partner_and_leader() {
        let st = linked(&[1], &[2]);
        assert_eq!(st.linked_partner(StripId::Ch(1)), Some(StripId::Ch(2)));
        assert_eq!(st.linked_partner(StripId::Ch(2)), Some(StripId::Ch(1)));
        assert_eq!(st.linked_partner(StripId::Ch(3)), None);
        assert_eq!(st.linked_partner(StripId::Bus(3)), Some(StripId::Bus(4)));
        assert_eq!(st.linked_partner(StripId::Main), None);
        assert_eq!(st.link_leader(StripId::Ch(2)), StripId::Ch(1));
        assert_eq!(st.link_leader(StripId::Ch(1)), StripId::Ch(1));
        assert_eq!(st.link_leader(StripId::Ch(3)), StripId::Ch(3));
    }

    #[test]
    fn display_collapses_linked_pairs_keeping_order() {
        let st = linked(&[1], &[]);
        let shown = st.display_strips(&[StripId::Ch(2), StripId::Ch(3), StripId::Ch(1)]);
        assert_eq!(shown, vec![StripId::Ch(1), StripId::Ch(3)]);
        assert_eq!(MixerState::default().display_strips(&[StripId::Ch(2), StripId::Ch(1)]), vec![StripId::Ch(2), StripId::Ch(1)]);
    }

    #[test]
    fn meters_read_the_right_slots() {
        let mut m = Meters::default();
        for (i, v) in m.m1.iter_mut().enumerate() {
            *v = -(i as f32);
        }
        let at = |i: usize| -(i as f32);
        assert_eq!(m.strip(StripId::Ch(5)), Some(at(m1::CH + 4)));
        assert_eq!(m.strip(StripId::Bus(2)), Some(at(m1::BUS + 1)));
        assert_eq!(m.strip(StripId::FxSend(1)), Some(at(m1::FX_SEND)));
        assert_eq!(m.strip(StripId::Dca(1)), None);
        // stereo strips report the louder side (values are negative: the larger one wins)
        assert_eq!(m.strip(StripId::Main), Some(at(m1::MAIN)));
        assert_eq!(m.stereo(StripId::Main), Some((at(m1::MAIN), at(m1::MAIN + 1))));
        assert_eq!(m.stereo(StripId::FxRtn(2)), Some((at(m1::FX_RTN + 2), at(m1::FX_RTN + 3))));
        assert_eq!(m.stereo(StripId::Ch(1)), None);
    }
}
