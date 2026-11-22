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
    /// Number keys pick the n-th strip of this bank instead of mixer channel n (default).
    #[serde(default = "default_true")]
    pub positional: bool,
}

fn default_true() -> bool {
    true
}

impl Bank {
    pub fn new(name: &str, strips: impl IntoIterator<Item = StripId>) -> Bank {
        Bank { name: name.into(), strips: strips.into_iter().collect(), bus: None, positional: true }
    }
}

/// The open show's banks plus the current one. Invariant: never empty, `current` in range, so
/// callers cannot index out of bounds or step through an empty list.
#[derive(Debug, Clone, PartialEq)]
pub struct BankSet {
    banks: Vec<Bank>,
    current: usize,
}

impl BankSet {
    /// An empty list falls back to the factory banks; `current` is clamped.
    pub fn new(banks: Vec<Bank>, current: usize) -> BankSet {
        let banks = if banks.is_empty() { defaults() } else { banks };
        let current = current.min(banks.len() - 1);
        BankSet { banks, current }
    }

    pub fn list(&self) -> &[Bank] {
        &self.banks
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.banks.len()
    }

    pub fn index(&self) -> usize {
        self.current
    }

    pub fn current(&self) -> &Bank {
        &self.banks[self.current]
    }

    /// Ignored when `i` is out of range.
    pub fn select(&mut self, i: usize) {
        if i < self.banks.len() {
            self.current = i;
        }
    }

    /// Move by `d` banks, wrapping around.
    pub fn step(&mut self, d: i32) {
        let n = self.banks.len() as i32;
        self.current = (self.current as i32 + d).rem_euclid(n) as usize;
    }

    /// Append and select; returns the new index.
    pub fn add(&mut self, bank: Bank) -> usize {
        self.banks.push(bank);
        self.current = self.banks.len() - 1;
        self.current
    }

    /// Refuses to delete the last bank (returns false).
    pub fn delete(&mut self, i: usize) -> bool {
        if self.banks.len() <= 1 || i >= self.banks.len() {
            return false;
        }
        self.banks.remove(i);
        self.current = self.current.min(self.banks.len() - 1);
        true
    }

    pub fn replace(&mut self, i: usize, bank: Bank) {
        if let Some(slot) = self.banks.get_mut(i) {
            *slot = bank;
        }
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

#[cfg(test)]
mod bankset_tests {
    use super::*;

    fn set(n: usize) -> BankSet {
        BankSet::new((0..n).map(|i| Bank::new(&format!("b{i}"), [])).collect(), 0)
    }

    #[test]
    fn never_empty_and_current_in_range() {
        let b = BankSet::new(vec![], 7);
        assert_eq!(b.len(), defaults().len());
        assert_eq!(b.index(), b.len() - 1);
    }

    #[test]
    fn step_wraps_and_select_ignores_out_of_range() {
        let mut b = set(3);
        b.step(-1);
        assert_eq!(b.index(), 2);
        b.step(2);
        assert_eq!(b.index(), 1);
        b.select(9);
        assert_eq!(b.index(), 1);
        b.select(0);
        assert_eq!(b.index(), 0);
    }

    #[test]
    fn add_selects_and_delete_keeps_the_invariant() {
        let mut b = set(2);
        assert_eq!(b.add(Bank::new("new", [])), 2);
        assert_eq!(b.index(), 2);
        assert!(b.delete(2));
        assert_eq!(b.index(), 1);
        assert!(b.delete(0));
        assert!(!b.delete(0), "last bank stays");
        assert_eq!((b.len(), b.index()), (1, 0));
        b.step(5);
        assert_eq!(b.index(), 0);
    }
}
