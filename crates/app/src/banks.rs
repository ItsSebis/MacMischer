//! Fader banks: user-defined lists of strips, optionally bound to a send bus.

use serde::{Deserialize, Serialize};
use xair_proto::{SendTo, StripId};

/// (De)serialize `Vec<StripId>` as `short()` strings; unknown entries are skipped on load.
pub mod strips_serde {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[StripId], s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(v.iter().map(|x| x.short()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<StripId>, D::Error> {
        let raw = Vec::<String>::deserialize(d)?;
        Ok(raw.iter().filter_map(|s| StripId::from_short(s)).collect())
    }
}

/// (De)serialize `Option<SendTo>` as "B1".."B6" / "FX1".."FX4"; unknown → None.
pub mod sendto_serde {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn to_str(t: SendTo) -> String {
        match t {
            SendTo::Bus(n) => format!("B{n}"),
            SendTo::Fx(n) => format!("FX{n}"),
        }
    }

    pub fn from_str(s: &str) -> Option<SendTo> {
        let s = s.trim().to_ascii_uppercase();
        if let Some(n) = s.strip_prefix("FX") {
            n.parse().ok().filter(|n| (1..=4).contains(n)).map(SendTo::Fx)
        } else if let Some(n) = s.strip_prefix('B') {
            n.parse().ok().filter(|n| (1..=6).contains(n)).map(SendTo::Bus)
        } else {
            None
        }
    }

    pub fn serialize<S: Serializer>(v: &Option<SendTo>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(t) => s.serialize_some(&to_str(*t)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SendTo>, D::Error> {
        Ok(Option::<String>::deserialize(d)?.and_then(|s| from_str(&s)))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bank {
    pub name: String,
    #[serde(default, with = "strips_serde")]
    pub strips: Vec<StripId>,
    /// When set, faders of input strips control their send to this bus/FX (sends-on-fader).
    #[serde(default, with = "sendto_serde", skip_serializing_if = "Option::is_none")]
    pub bus: Option<SendTo>,
    /// Number keys pick the n-th strip of this bank instead of mixer channel n.
    #[serde(default)]
    pub positional: bool,
}

impl Bank {
    pub fn new(name: &str, strips: impl IntoIterator<Item = StripId>) -> Bank {
        Bank { name: name.into(), strips: strips.into_iter().collect(), bus: None, positional: false }
    }
}

/// Factory banks until show files (M5) take over.
pub fn defaults() -> Vec<Bank> {
    use StripId::*;
    vec![
        Bank::new("Inputs 1–8", (1..=8).map(Ch)),
        Bank::new("Inputs 9–16", (9..=16).map(Ch)),
        Bank::new("Aux / FX", [AuxIn, FxRtn(1), FxRtn(2), FxRtn(3), FxRtn(4)]),
        Bank::new("Buses", (1..=6).map(Bus).chain((1..=4).map(FxSend))),
        Bank::new("DCA", (1..=4).map(Dca)),
    ]
}
