//! Typed mixer parameters <-> OSC address paths.

use crate::routing::Output;
use crate::proc::{self, EqField, Proc};
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
    /// int RTA analysis source, see `meters::rta_source_label` (0..=32; hardware-verified on an XR18)
    RtaSource,
    /// int RTA tap point: 0 = PRE (before the strip's EQ/dynamics/fader), 1 = POST (hardware-verified)
    RtaPos,
    /// int current snapshot index
    SnapIndex,
    /// string snapshot name 1..64
    SnapName(u8),
    /// int 0..15 analog input In01..In16 (channels only)
    InSrc(StripId),
    /// int 0..17 USB return U01..U18 (channels); 0..8 USB pair U01-02..U17-18 (aux in)
    RtnSrc(StripId),
    /// int 0 = analog input, 1 = USB return (channels and aux in)
    RtnSw(StripId),
    /// int source code, see `routing::source_label`
    RouteSrc(Output),
    /// int tap code, see `routing::tap_label` (outputs with `has_pos`)
    RoutePos(Output),
    /// channel processing (HPF, EQ, gate, compressor); see `proc`
    Proc(StripId, Proc),
    /// int effect type code of FX slot 1..4 (type names/order not hardware-verified, see PROTOCOL.md)
    FxType(u8),
    /// float 0..1 raw effect parameter `par` (1..=FX_PARAMS) of FX slot 1..4; meaning depends on type
    FxPar(u8, u8),
}

/// Effect parameters mirrored per FX slot (the mixer has up to 64; most effects use far fewer).
pub const FX_PARAMS: u8 = 32;

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
            Param::RtaSource => "/-stat/rta/source".into(),
            Param::RtaPos => "/-stat/rta/pos".into(),
            Param::SnapIndex => "/-snap/index".into(),
            Param::SnapName(i) => format!("/-snap/{i:02}/name"),
            Param::InSrc(s) => format!("{}/config/insrc", s.prefix()),
            Param::RtnSrc(s) => format!("{}/config/rtnsrc", s.prefix()),
            Param::RtnSw(s) => format!("{}/preamp/rtnsw", s.prefix()),
            Param::RouteSrc(o) => format!("{}/src", o.prefix()),
            Param::RoutePos(o) => format!("{}/pos", o.prefix()),
            Param::Proc(s, p) => format!("{}{}", s.prefix(), p.suffix()),
            Param::FxType(n) => format!("/fx/{n}/type"),
            Param::FxPar(n, i) => format!("/fx/{n}/par/{i:02}"),
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
                "/config/insrc" if matches!(strip, StripId::Ch(_)) => Some(Param::InSrc(strip)),
                "/config/rtnsrc" if matches!(strip, StripId::Ch(_) | StripId::AuxIn) => {
                    Some(Param::RtnSrc(strip))
                }
                "/preamp/rtnsw" if matches!(strip, StripId::Ch(_) | StripId::AuxIn) => {
                    Some(Param::RtnSw(strip))
                }
                _ => {
                    if let Some(p) = Proc::parse_suffix(rest) {
                        return p.valid_for(strip).then_some(Param::Proc(strip, p));
                    }
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
            ["", "-stat", "rta", "source"] => Some(Param::RtaSource),
            ["", "-stat", "rta", "pos"] => Some(Param::RtaPos),
            ["", "-snap", "index"] => Some(Param::SnapIndex),
            ["", "config", "mute", n] => Some(Param::MuteGroupOn(n.parse().ok()?)),
            ["", "config", "solo", "level"] => Some(Param::MonitorLevel),
            ["", "config", "chlink", pair] => Some(Param::ChLink(link_pair(pair)?)),
            ["", "config", "buslink", pair] => Some(Param::BusLink(link_pair(pair)?)),
            ["", "fx", n, "type"] => Some(Param::FxType(fx_slot(n)?)),
            ["", "fx", n, "par", i] => {
                let i: u8 = i.parse().ok()?;
                (1..=FX_PARAMS).contains(&i).then_some(Param::FxPar(fx_slot(n)?, i))
            }
            ["", "-snap", n, "name"] => Some(Param::SnapName(n.parse().ok()?)),
            ["", "routing", kind, n, leaf] => {
                let o = Output::parse_prefix(kind, n)?;
                match *leaf {
                    "src" => Some(Param::RouteSrc(o)),
                    "pos" if o.has_pos() => Some(Param::RoutePos(o)),
                    _ => None,
                }
            }
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

    /// The level parameter a fader control acts on: the send to `bus` in a bus-bound bank (for
    /// strips that have sends), else the strip's own fader.
    pub fn level(s: StripId, bus: Option<SendTo>) -> Param {
        match bus {
            Some(t) if s.has_sends() => Param::SendLevel(s, t),
            _ => Param::Fader(s),
        }
    }

    /// Whether the Lock lets this write through: mutes, solo, mute groups and the main fader.
    /// One rule for every client (app, MIDI, web remote).
    pub fn allowed_when_locked(self) -> bool {
        matches!(self, Param::On(_) | Param::Solo(_) | Param::MuteGroupOn(_) | Param::Fader(StripId::Main))
    }

    /// The strip this parameter belongs to, if any.
    pub fn strip(self) -> Option<StripId> {
        match self {
            Param::Solo(n) => StripId::from_solo_index(n),
            Param::Fader(s) | Param::On(s) | Param::Pan(s) | Param::Name(s) | Param::Color(s)
            | Param::DcaGroups(s) | Param::MuteGroups(s) | Param::MainAssign(s)
            | Param::SendLevel(s, _) | Param::SendTap(s, _) | Param::Proc(s, _) => Some(s),
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
        for s in StripId::all() {
            v.extend(proc_params(s).into_iter().map(|p| Param::Proc(s, p)));
        }
        for h in 1..=18 {
            v.push(Param::Gain(h));
            if crate::strip::headamp_has_phantom(h) {
                v.push(Param::Phantom(h));
            }
        }
        for n in 1..=crate::strip::MUTE_GROUPS {
            v.push(Param::MuteGroupOn(n));
        }
        v.push(Param::MonitorLevel);
        v.extend((1..=crate::strip::CHANNELS / 2).map(Param::ChLink));
        v.extend((1..=BUSES / 2).map(Param::BusLink));
        for c in 1..=crate::strip::CHANNELS {
            v.push(Param::InSrc(StripId::Ch(c)));
            v.push(Param::RtnSrc(StripId::Ch(c)));
            v.push(Param::RtnSw(StripId::Ch(c)));
        }
        v.push(Param::RtnSrc(StripId::AuxIn));
        v.push(Param::RtnSw(StripId::AuxIn));
        for o in [Output::Main(1), Output::Aux(1), Output::Usb(1), Output::P16(1)] {
            for o in o.all_of_kind() {
                v.push(Param::RouteSrc(o));
                if o.has_pos() {
                    v.push(Param::RoutePos(o));
                }
            }
        }
        for n in 1..=FX {
            v.push(Param::FxType(n));
            v.extend((1..=FX_PARAMS).map(|i| Param::FxPar(n, i)));
        }
        v.push(Param::RtaSource);
        v.push(Param::RtaPos);
        v.push(Param::SnapIndex);
        for i in 1..=64 {
            v.push(Param::SnapName(i));
        }
        v
    }
}

/// Every processing parameter that exists on strip `s`.
pub fn proc_params(s: StripId) -> Vec<Proc> {
    let mut v = Vec::new();
    for (p, _) in proc::all_fixed() {
        if p.valid_for(s) && !matches!(p, Proc::EqOn) {
            v.push(p);
        }
    }
    if proc::eq_bands(s) > 0 {
        v.push(Proc::EqOn);
        for b in 1..=proc::eq_bands(s) {
            v.extend(EqField::ALL.map(|f| Proc::Eq(b, f)));
        }
    }
    v
}

fn fx_slot(s: &str) -> Option<u8> {
    s.parse().ok().filter(|n| (1..=FX).contains(n))
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
        assert_eq!(Param::InSrc(StripId::Ch(5)).path(), "/ch/05/config/insrc");
        assert_eq!(Param::RtnSw(StripId::AuxIn).path(), "/rtn/aux/preamp/rtnsw");
        assert_eq!(Param::RouteSrc(Output::Usb(3)).path(), "/routing/usb/03/src");
        assert_eq!(Param::parse("/routing/main/01/pos"), None);
        assert_eq!(Param::parse("/routing/aux/06/pos"), Some(Param::RoutePos(Output::Aux(6))));
        assert_eq!(Param::parse("/rtn/aux/config/insrc"), None);
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
    fn proc_paths() {
        let p = Param::Proc(StripId::Bus(2), Proc::Eq(6, EqField::Q));
        assert_eq!(p.path(), "/bus/2/eq/6/q");
        assert_eq!(Param::parse("/ch/03/gate/filter/f"), Some(Param::Proc(StripId::Ch(3), Proc::GateFilterF)));
        assert_eq!(Param::parse("/lr/dyn/ratio"), Some(Param::Proc(StripId::Main, Proc::DynRatio)));
    }

    #[test]
    fn fx_paths() {
        assert_eq!(Param::FxType(2).path(), "/fx/2/type");
        assert_eq!(Param::FxPar(4, 7).path(), "/fx/4/par/07");
        assert_eq!(Param::parse("/fx/1/par/32"), Some(Param::FxPar(1, 32)));
        assert_eq!(Param::parse("/fx/5/type"), None);
        assert_eq!(Param::parse("/fx/1/par/33"), None);
    }

    #[test]
    fn unknown_paths() {
        assert_eq!(Param::parse("/ch/01/eq/5/f"), None);
        assert_eq!(Param::parse("/fxsend/1/eq/1/f"), None);
        assert_eq!(Param::parse("/bus/1/gate/on"), None);
        assert_eq!(Param::parse("/dca/1/mix/fader"), None);
        assert_eq!(Param::parse("/ch/01/mix/11/level"), None);
    }
}

#[cfg(test)]
mod shared_rules_tests {
    use super::*;

    #[test]
    fn level_param_follows_the_bank_bus() {
        let t = SendTo::Bus(2);
        assert_eq!(Param::level(StripId::Ch(3), None), Param::Fader(StripId::Ch(3)));
        assert_eq!(Param::level(StripId::Ch(3), Some(t)), Param::SendLevel(StripId::Ch(3), t));
        // strips without sends keep their own fader
        assert_eq!(Param::level(StripId::Main, Some(t)), Param::Fader(StripId::Main));
    }

    #[test]
    fn lock_rule_is_mutes_solo_groups_and_main_fader() {
        let ch = StripId::Ch(1);
        assert!(Param::On(ch).allowed_when_locked());
        assert!(Param::Solo(ch.solo_index()).allowed_when_locked());
        assert!(Param::Fader(StripId::Main).allowed_when_locked());
        assert!(!Param::Fader(ch).allowed_when_locked());
        assert!(!Param::SendLevel(ch, SendTo::Bus(1)).allowed_when_locked());
    }

    #[test]
    fn solo_index_round_trips() {
        for s in StripId::all() {
            assert_eq!(StripId::from_solo_index(s.solo_index()), Some(s));
        }
    }
}
