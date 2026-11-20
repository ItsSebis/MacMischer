//! Typed mixer parameters <-> OSC address paths.

use crate::strip::{StripId, BUSES, FX};

/// A value as carried over OSC.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Float(f32),
    Int(i32),
    Str(String),
}

impl Value {
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f32),
            Value::Str(_) => None,
        }
    }
    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f) => Some(*f as i32),
            Value::Str(_) => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// Send target from an input strip: buses 1–6 (`mix/01..06`) or FX sends 1–4 (`mix/07..10`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SendTo {
    Bus(u8),
    Fx(u8),
}

impl SendTo {
    fn index(self) -> u8 {
        match self {
            SendTo::Bus(n) => n,
            SendTo::Fx(n) => BUSES + n,
        }
    }
    fn from_index(i: u8) -> Option<SendTo> {
        match i {
            1..=BUSES => Some(SendTo::Bus(i)),
            _ if i > BUSES && i <= BUSES + FX => Some(SendTo::Fx(i - BUSES)),
            _ => None,
        }
    }
    /// The strip whose master this send feeds.
    pub fn strip(self) -> StripId {
        match self {
            SendTo::Bus(n) => StripId::Bus(n),
            SendTo::Fx(n) => StripId::FxSend(n),
        }
    }
    pub fn all() -> impl Iterator<Item = SendTo> {
        (1..=BUSES + FX).filter_map(SendTo::from_index)
    }
}

/// Every parameter the app reads or writes. Extend here as features grow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Param {
    /// float 0..1 (see `fader`)
    Fader(StripId),
    /// int 1 = on (unmuted), 0 = muted
    On(StripId),
    /// float 0..1, 0.5 = centre
    Pan(StripId),
    /// string, max 12 chars
    Name(StripId),
    /// int 0..15
    Color(StripId),
    /// int bitmask of DCA groups, bit 0 = DCA 1 (inputs, buses, FX sends)
    DcaGroups(StripId),
    /// int bitmask of mute groups, bit 0 = group 1 (inputs, buses, FX sends)
    MuteGroups(StripId),
    /// int 1 = mute group n (1..4) engaged
    MuteGroupOn(u8),
    /// float 0..1, fader law — monitor / phones level
    MonitorLevel,
    /// int 1 = channels 2k-1/2k stereo-linked (pair k = 1..8)
    ChLink(u8),
    /// int 1 = buses 2k-1/2k stereo-linked (pair k = 1..3)
    BusLink(u8),
    /// int 1 = assigned to main LR
    MainAssign(StripId),
    /// float 0..1, fader law
    SendLevel(StripId, SendTo),
    /// int tap point code
    SendTap(StripId, SendTo),
    /// float 0..1 (headamp 1..18)
    Gain(u8),
    /// int 0/1 (headamp 1..18)
    Phantom(u8),
    /// int 0/1 — solo switch by mixer solo index (see PROTOCOL.md; mapping TBD)
    Solo(u8),
    /// int current snapshot index
    SnapIndex,
    /// string snapshot name 1..64
    SnapName(u8),
}

impl Param {
    pub fn path(self) -> String {
        match self {
            Param::Fader(s @ StripId::Dca(_)) => format!("{}/fader", s.prefix()),
            Param::On(s @ StripId::Dca(_)) => format!("{}/on", s.prefix()),
            Param::Fader(s) => format!("{}/mix/fader", s.prefix()),
            Param::On(s) => format!("{}/mix/on", s.prefix()),
            Param::Pan(s) => format!("{}/mix/pan", s.prefix()),
            Param::Name(s) => format!("{}/config/name", s.prefix()),
            Param::Color(s) => format!("{}/config/color", s.prefix()),
            Param::DcaGroups(s) => format!("{}/grp/dca", s.prefix()),
            Param::MuteGroups(s) => format!("{}/grp/mute", s.prefix()),
            Param::MuteGroupOn(n) => format!("/config/mute/{n}"),
            Param::MonitorLevel => "/config/solo/level".into(),
            Param::ChLink(k) => format!("/config/chlink/{}-{}", 2 * k - 1, 2 * k),
            Param::BusLink(k) => format!("/config/buslink/{}-{}", 2 * k - 1, 2 * k),
            Param::MainAssign(s) => format!("{}/mix/lr", s.prefix()),
            Param::SendLevel(s, t) => format!("{}/mix/{:02}/level", s.prefix(), t.index()),
            Param::SendTap(s, t) => format!("{}/mix/{:02}/tap", s.prefix(), t.index()),
            Param::Gain(h) => format!("/headamp/{h:02}/gain"),
            Param::Phantom(h) => format!("/headamp/{h:02}/phantom"),
            Param::Solo(i) => format!("/-stat/solosw/{i:02}"),
            Param::SnapIndex => "/-snap/index".into(),
            Param::SnapName(i) => format!("/-snap/{i:02}/name"),
        }
    }

    pub fn parse(path: &str) -> Option<Param> {
        if let Some((strip, rest)) = StripId::parse_prefix(path) {
            let is_dca = matches!(strip, StripId::Dca(_));
            return match rest {
                "/fader" if is_dca => Some(Param::Fader(strip)),
                "/on" if is_dca => Some(Param::On(strip)),
                "/mix/fader" if !is_dca => Some(Param::Fader(strip)),
                "/mix/on" if !is_dca => Some(Param::On(strip)),
                "/mix/pan" => Some(Param::Pan(strip)),
                "/config/name" => Some(Param::Name(strip)),
                "/config/color" => Some(Param::Color(strip)),
                "/grp/dca" => Some(Param::DcaGroups(strip)),
                "/grp/mute" => Some(Param::MuteGroups(strip)),
                "/mix/lr" => Some(Param::MainAssign(strip)),
                _ => {
                    let r = rest.strip_prefix("/mix/")?;
                    let (idx, leaf) = r.split_once('/')?;
                    let to = SendTo::from_index(idx.parse().ok()?)?;
                    match leaf {
                        "level" => Some(Param::SendLevel(strip, to)),
                        "tap" => Some(Param::SendTap(strip, to)),
                        _ => None,
                    }
                }
            };
        }
        let parts: Vec<&str> = path.split('/').collect();
        match parts.as_slice() {
            ["", "headamp", n, "gain"] => Some(Param::Gain(n.parse().ok()?)),
            ["", "headamp", n, "phantom"] => Some(Param::Phantom(n.parse().ok()?)),
            ["", "-stat", "solosw", n] => Some(Param::Solo(n.parse().ok()?)),
            ["", "-snap", "index"] => Some(Param::SnapIndex),
            ["", "config", "mute", n] => Some(Param::MuteGroupOn(n.parse().ok()?)),
            ["", "config", "solo", "level"] => Some(Param::MonitorLevel),
            ["", "config", "chlink", pair] => Some(Param::ChLink(link_pair(pair)?)),
            ["", "config", "buslink", pair] => Some(Param::BusLink(link_pair(pair)?)),
            ["", "-snap", n, "name"] => Some(Param::SnapName(n.parse().ok()?)),
            _ => None,
        }
    }

    /// The same parameter on another strip (for mirroring to a linked partner).
    pub fn with_strip(self, to: StripId) -> Option<Param> {
        Some(match self {
            Param::Fader(_) => Param::Fader(to),
            Param::On(_) => Param::On(to),
            Param::SendLevel(_, t) => Param::SendLevel(to, t),
            Param::DcaGroups(_) => Param::DcaGroups(to),
            Param::MuteGroups(_) => Param::MuteGroups(to),
            Param::Solo(_) => Param::Solo(to.solo_index()),
            _ => return None,
        })
    }

    /// The strip this parameter belongs to, if any.
    pub fn strip(self) -> Option<StripId> {
        match self {
            Param::Fader(s) | Param::On(s) | Param::Pan(s) | Param::Name(s) | Param::Color(s)
            | Param::DcaGroups(s) | Param::MuteGroups(s) | Param::MainAssign(s)
            | Param::SendLevel(s, _) | Param::SendTap(s, _) => Some(s),
            _ => None,
        }
    }

    /// The full set of parameters to read when syncing state on connect.
    pub fn sync_set() -> Vec<Param> {
        let mut v = Vec::new();
        for s in StripId::all() {
            v.push(Param::Fader(s));
            v.push(Param::On(s));
            v.push(Param::Name(s));
            v.push(Param::Color(s));
            v.push(Param::Solo(s.solo_index()));
            if s.has_pan() {
                v.push(Param::Pan(s));
            }
            if s.has_groups() {
                v.push(Param::DcaGroups(s));
                v.push(Param::MuteGroups(s));
            }
            if s.has_sends() {
                v.push(Param::MainAssign(s));
                for t in SendTo::all() {
                    v.push(Param::SendLevel(s, t));
                    if matches!(s, StripId::Ch(_) | StripId::AuxIn) {
                        v.push(Param::SendTap(s, t));
                    }
                }
            }
        }
        for h in 1..=18 {
            v.push(Param::Gain(h));
            if h <= 16 {
                v.push(Param::Phantom(h)); // aux in 17/18 has no phantom
            }
        }
        for n in 1..=crate::strip::MUTE_GROUPS {
            v.push(Param::MuteGroupOn(n));
        }
        v.push(Param::MonitorLevel);
        v.extend((1..=crate::strip::CHANNELS / 2).map(Param::ChLink));
        v.extend((1..=BUSES / 2).map(Param::BusLink));
        v.push(Param::SnapIndex);
        for i in 1..=64 {
            v.push(Param::SnapName(i));
        }
        v
    }
}

/// "3-4" → pair 2.
fn link_pair(s: &str) -> Option<u8> {
    let (a, b) = s.split_once('-')?;
    let (a, b): (u8, u8) = (a.parse().ok()?, b.parse().ok()?);
    (a % 2 == 1 && b == a + 1).then_some(a.div_ceil(2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_paths() {
        assert_eq!(Param::Fader(StripId::Ch(1)).path(), "/ch/01/mix/fader");
        assert_eq!(Param::Fader(StripId::Dca(2)).path(), "/dca/2/fader");
        assert_eq!(Param::On(StripId::Main).path(), "/lr/mix/on");
        assert_eq!(
            Param::SendLevel(StripId::Ch(3), SendTo::Fx(2)).path(),
            "/ch/03/mix/08/level"
        );
        assert_eq!(Param::Gain(1).path(), "/headamp/01/gain");
        assert_eq!(Param::SnapName(2).path(), "/-snap/02/name");
        assert_eq!(Param::ChLink(2).path(), "/config/chlink/3-4");
        assert_eq!(Param::parse("/config/buslink/5-6"), Some(Param::BusLink(3)));
        assert_eq!(Param::parse("/config/chlink/2-3"), None);
    }

    #[test]
    fn sync_set_round_trips() {
        let set = Param::sync_set();
        assert!(set.len() > 300);
        for p in set {
            assert_eq!(Param::parse(&p.path()), Some(p), "{}", p.path());
        }
    }

    #[test]
    fn unknown_paths() {
        assert_eq!(Param::parse("/ch/01/eq/1/f"), None);
        assert_eq!(Param::parse("/dca/1/mix/fader"), None);
        assert_eq!(Param::parse("/ch/01/mix/11/level"), None);
    }
}
