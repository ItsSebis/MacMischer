//! Channel-processing parameters (HPF, gate, EQ, compressor), addressed relative to a strip prefix.

use crate::scale::{self, Scale};
use crate::strip::StripId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EqField {
    Type,
    F,
    G,
    Q,
}

impl EqField {
    pub const ALL: [EqField; 4] = [EqField::Type, EqField::F, EqField::G, EqField::Q];
    fn leaf(self) -> &'static str {
        match self {
            EqField::Type => "type",
            EqField::F => "f",
            EqField::G => "g",
            EqField::Q => "q",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Proc {
    HpOn,
    Hpf,
    Invert,
    EqOn,
    /// band 1-based (1..=4 or 1..=6)
    Eq(u8, EqField),
    GateOn,
    GateMode,
    GateThr,
    GateRange,
    GateAttack,
    GateHold,
    GateRelease,
    GateKeySrc,
    GateFilterOn,
    GateFilterType,
    GateFilterF,
    DynOn,
    DynMode,
    DynDet,
    DynEnv,
    DynThr,
    DynRatio,
    DynKnee,
    DynMgain,
    DynAttack,
    DynHold,
    DynRelease,
    DynMix,
    DynAuto,
    DynKeySrc,
    DynFilterOn,
    DynFilterType,
    DynFilterF,
}

/// Non-EQ suffixes in one table: (variant, suffix).
const FIXED: [(Proc, &str); 31] = [
    (Proc::HpOn, "/preamp/hpon"),
    (Proc::Hpf, "/preamp/hpf"),
    (Proc::Invert, "/preamp/invert"),
    (Proc::EqOn, "/eq/on"),
    (Proc::GateOn, "/gate/on"),
    (Proc::GateMode, "/gate/mode"),
    (Proc::GateThr, "/gate/thr"),
    (Proc::GateRange, "/gate/range"),
    (Proc::GateAttack, "/gate/attack"),
    (Proc::GateHold, "/gate/hold"),
    (Proc::GateRelease, "/gate/release"),
    (Proc::GateKeySrc, "/gate/keysrc"),
    (Proc::GateFilterOn, "/gate/filter/on"),
    (Proc::GateFilterType, "/gate/filter/type"),
    (Proc::GateFilterF, "/gate/filter/f"),
    (Proc::DynOn, "/dyn/on"),
    (Proc::DynMode, "/dyn/mode"),
    (Proc::DynDet, "/dyn/det"),
    (Proc::DynEnv, "/dyn/env"),
    (Proc::DynThr, "/dyn/thr"),
    (Proc::DynRatio, "/dyn/ratio"),
    (Proc::DynKnee, "/dyn/knee"),
    (Proc::DynMgain, "/dyn/mgain"),
    (Proc::DynAttack, "/dyn/attack"),
    (Proc::DynHold, "/dyn/hold"),
    (Proc::DynRelease, "/dyn/release"),
    (Proc::DynMix, "/dyn/mix"),
    (Proc::DynAuto, "/dyn/auto"),
    (Proc::DynKeySrc, "/dyn/keysrc"),
    (Proc::DynFilterOn, "/dyn/filter/on"),
    (Proc::DynFilterType, "/dyn/filter/type"),
];

/// All non-EQ-band params (including `EqOn` and `DynFilterF`).
pub fn all_fixed() -> impl Iterator<Item = (Proc, String)> {
    FIXED.iter().map(|(p, s)| (*p, s.to_string())).chain([(Proc::DynFilterF, "/dyn/filter/f".to_string())])
}

pub const EQ_TYPES: [&str; 6] = ["LCut", "LShv", "PEQ", "VEQ", "HShv", "HCut"];
pub const GATE_MODES: [&str; 5] = ["EXP2", "EXP3", "EXP4", "GATE", "DUCK"];
pub const RATIOS: [f32; 12] = [1.1, 1.3, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 7.0, 10.0, 20.0, 100.0];
pub const FILTER_TYPES: [&str; 9] = ["LC6", "LC12", "HC6", "HC12", "1.0", "2.0", "3.0", "5.0", "10.0"];
pub const DYN_MODES: [&str; 2] = ["COMP", "EXP"];
pub const DYN_DETS: [&str; 2] = ["PEAK", "RMS"];
pub const DYN_ENVS: [&str; 2] = ["LIN", "LOG"];

/// Number of EQ bands (0 for strips without EQ).
pub fn eq_bands(s: StripId) -> u8 {
    match s {
        StripId::Ch(_) | StripId::AuxIn | StripId::FxRtn(_) => 4,
        StripId::Bus(_) | StripId::Main => 6,
        StripId::FxSend(_) | StripId::Dca(_) => 0,
    }
}
pub fn has_gate(s: StripId) -> bool {
    matches!(s, StripId::Ch(_))
}
pub fn has_dyn(s: StripId) -> bool {
    matches!(s, StripId::Ch(_) | StripId::Bus(_) | StripId::Main)
}
pub fn has_hpf(s: StripId) -> bool {
    matches!(s, StripId::Ch(_))
}

impl Proc {
    /// Path suffix relative to the strip prefix, e.g. `/eq/2/f`.
    pub fn suffix(self) -> String {
        match self {
            Proc::Eq(b, f) => format!("/eq/{b}/{}", f.leaf()),
            Proc::DynFilterF => "/dyn/filter/f".into(),
            p => FIXED.iter().find(|(q, _)| *q == p).map(|(_, s)| s.to_string()).unwrap_or_default(),
        }
    }

    pub fn parse_suffix(s: &str) -> Option<Proc> {
        if s == "/dyn/filter/f" {
            return Some(Proc::DynFilterF);
        }
        if let Some(p) = FIXED.iter().find(|(_, x)| *x == s) {
            return Some(p.0);
        }
        if let Some(r) = s.strip_prefix("/eq/") {
            let (b, leaf) = r.split_once('/')?;
            if b.len() != 1 {
                return None;
            }
            let b: u8 = b.parse().ok()?;
            if !(1..=6).contains(&b) {
                return None;
            }
            let f = EqField::ALL.iter().copied().find(|f| f.leaf() == leaf)?;
            return Some(Proc::Eq(b, f));
        }
        None
    }

    /// Whether this parameter exists on the strip.
    pub fn valid_for(self, s: StripId) -> bool {
        match self {
            Proc::HpOn | Proc::Hpf | Proc::Invert => has_hpf(s),
            Proc::EqOn => eq_bands(s) > 0,
            Proc::Eq(b, _) => b >= 1 && b <= eq_bands(s),
            Proc::GateOn | Proc::GateMode | Proc::GateThr | Proc::GateRange | Proc::GateAttack
            | Proc::GateHold | Proc::GateRelease | Proc::GateKeySrc | Proc::GateFilterOn
            | Proc::GateFilterType | Proc::GateFilterF => has_gate(s),
            Proc::DynKeySrc => has_dyn(s) && s != StripId::Main, // /lr/dyn/keysrc unanswered by hardware
            _ => has_dyn(s),
        }
    }

    /// Integer-valued (switch / enum) parameter; everything else is a float.
    pub fn is_int(self) -> bool {
        matches!(
            self,
            Proc::HpOn | Proc::Invert | Proc::EqOn | Proc::Eq(_, EqField::Type)
                | Proc::GateOn | Proc::GateMode | Proc::GateKeySrc | Proc::GateFilterOn
                | Proc::GateFilterType | Proc::DynOn | Proc::DynMode | Proc::DynDet
                | Proc::DynEnv | Proc::DynRatio | Proc::DynAuto | Proc::DynKeySrc
                | Proc::DynFilterOn | Proc::DynFilterType
        )
    }

    /// Real-world scale of a float parameter.
    pub fn scale(self) -> Option<Scale> {
        Some(match self {
            Proc::Hpf => scale::HPF,
            Proc::Eq(_, EqField::F) | Proc::GateFilterF | Proc::DynFilterF => scale::FREQ,
            Proc::Eq(_, EqField::G) => scale::EQ_GAIN,
            Proc::Eq(_, EqField::Q) => scale::EQ_Q,
            Proc::GateThr => Scale::Lin { min: -80.0, max: 0.0 },
            Proc::GateRange => Scale::Lin { min: 3.0, max: 60.0 },
            Proc::GateAttack | Proc::DynAttack => scale::ATTACK,
            Proc::GateHold | Proc::DynHold => scale::HOLD,
            Proc::GateRelease | Proc::DynRelease => scale::RELEASE,
            Proc::DynThr => Scale::Lin { min: -60.0, max: 0.0 },
            Proc::DynKnee => Scale::Lin { min: 0.0, max: 5.0 },
            Proc::DynMgain => Scale::Lin { min: 0.0, max: 24.0 },
            Proc::DynMix => Scale::Lin { min: 0.0, max: 100.0 },
            _ => return None,
        })
    }

    /// Hardware quantisation steps of a float parameter, if coarse.
    pub fn steps(self) -> Option<u32> {
        match self {
            Proc::Eq(_, EqField::F) | Proc::GateFilterF | Proc::DynFilterF => Some(201),
            Proc::Eq(_, EqField::G) => Some(61),
            Proc::Eq(_, EqField::Q) => Some(72),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_round_trip() {
        for (p, _) in FIXED {
            assert_eq!(Proc::parse_suffix(&p.suffix()), Some(p));
        }
        assert_eq!(Proc::parse_suffix(&Proc::DynFilterF.suffix()), Some(Proc::DynFilterF));
        for b in 1..=6 {
            for f in EqField::ALL {
                let p = Proc::Eq(b, f);
                assert_eq!(Proc::parse_suffix(&p.suffix()), Some(p));
            }
        }
        assert_eq!(Proc::Eq(2, EqField::F).suffix(), "/eq/2/f");
        assert_eq!(Proc::parse_suffix("/eq/7/f"), None);
        assert_eq!(Proc::parse_suffix("/eq/1/x"), None);
        assert_eq!(Proc::parse_suffix("/preamp/hpslope"), None);
    }

    #[test]
    fn floats_have_scale_except_none() {
        assert!(Proc::GateThr.scale().is_some());
        assert!(Proc::GateMode.is_int());
        assert!(Proc::DynRatio.scale().is_none());
    }
}
