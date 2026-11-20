//! Fader banks: user-defined lists of strips, optionally bound to a send bus.

use xair_proto::{SendTo, StripId};

#[derive(Debug, Clone, PartialEq)]
pub struct Bank {
    pub name: String,
    pub strips: Vec<StripId>,
    /// When set, faders of input strips control their send to this bus/FX (sends-on-fader).
    pub bus: Option<SendTo>,
    /// Number keys pick the n-th strip of this bank instead of mixer channel n.
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
