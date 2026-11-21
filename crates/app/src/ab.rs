//! A/B compare: two snapshots of a strip's processing parameters, swapped by writing values.

use std::collections::HashMap;
use xair_proto::{Param, StripId, Value};

pub type Snapshot = Vec<(Param, Value)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    pub fn other(self) -> Slot {
        match self {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        }
    }
}

#[derive(Debug, Clone)]
struct Pair {
    a: Snapshot,
    b: Snapshot,
    active: Slot,
}

impl Pair {
    fn slot_mut(&mut self, s: Slot) -> &mut Snapshot {
        match s {
            Slot::A => &mut self.a,
            Slot::B => &mut self.b,
        }
    }
}

/// Per-strip A/B slots (app-side only, cleared on mixer switch).
#[derive(Debug, Default)]
pub struct Ab {
    strips: HashMap<StripId, Pair>,
}

impl Ab {
    /// The slot currently loaded for this strip, if A/B was started for it.
    pub fn active(&self, s: StripId) -> Option<Slot> {
        self.strips.get(&s).map(|p| p.active)
    }

    pub fn clear(&mut self) {
        self.strips.clear();
    }

    /// Switch strip `s` to `target`. `live` is the strip's current processing state.
    /// Returns the values to write (only those that differ from live).
    /// The first call captures `live` into both slots and makes `target` active.
    pub fn select(&mut self, s: StripId, target: Slot, live: Snapshot) -> Snapshot {
        let Some(pair) = self.strips.get_mut(&s) else {
            self.strips.insert(s, Pair { a: live.clone(), b: live, active: target });
            return Vec::new();
        };
        if pair.active == target {
            return Vec::new();
        }
        let left = pair.active;
        *pair.slot_mut(left) = live.clone();
        pair.active = target;
        let now: HashMap<Param, &Value> = live.iter().map(|(p, v)| (*p, v)).collect();
        pair.slot_mut(target)
            .iter()
            .filter(|(p, v)| now.get(p).is_none_or(|cur| *cur != v))
            .cloned()
            .collect()
    }

    /// Toggle to the other slot; the first press starts A/B with slot A active.
    pub fn toggle(&mut self, s: StripId, live: Snapshot) -> Snapshot {
        let target = self.active(s).map_or(Slot::A, Slot::other);
        self.select(s, target, live)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xair_proto::Proc;

    fn snap(ch: u8, hpf: f32, thr: f32) -> Snapshot {
        let s = StripId::Ch(ch);
        vec![(Param::Proc(s, Proc::Hpf), Value::Float(hpf)), (Param::Proc(s, Proc::HpOn), Value::Int(1)), (Param::Fader(s), Value::Float(thr))]
    }

    #[test]
    fn first_press_captures_both_without_writes() {
        let mut ab = Ab::default();
        let s = StripId::Ch(1);
        assert_eq!(ab.active(s), None);
        assert!(ab.toggle(s, snap(1, 0.1, 0.2)).is_empty());
        assert_eq!(ab.active(s), Some(Slot::A));
        let p = &ab.strips[&s];
        assert_eq!(p.a, p.b);
    }

    #[test]
    fn toggle_restores_and_preserves_edits() {
        let mut ab = Ab::default();
        let s = StripId::Ch(1);
        ab.toggle(s, snap(1, 0.1, 0.2));
        // edit while on A, then go to B: B still holds the captured values
        let w = ab.toggle(s, snap(1, 0.5, 0.2));
        assert_eq!(w, vec![(Param::Proc(s, Proc::Hpf), Value::Float(0.1))]);
        assert_eq!(ab.active(s), Some(Slot::B));
        // edit on B, back to A: A has the edited 0.5, B's edit is stored
        let w = ab.toggle(s, snap(1, 0.9, 0.2));
        assert_eq!(w, vec![(Param::Proc(s, Proc::Hpf), Value::Float(0.5))]);
        let w = ab.toggle(s, snap(1, 0.5, 0.2));
        assert_eq!(w, vec![(Param::Proc(s, Proc::Hpf), Value::Float(0.9))]);
    }

    #[test]
    fn select_same_slot_is_noop_and_first_select_b() {
        let mut ab = Ab::default();
        let s = StripId::Ch(1);
        assert!(ab.select(s, Slot::B, snap(1, 0.1, 0.2)).is_empty());
        assert_eq!(ab.active(s), Some(Slot::B));
        assert!(ab.select(s, Slot::B, snap(1, 0.7, 0.2)).is_empty());
    }

    #[test]
    fn strips_are_isolated_and_clear_resets() {
        let mut ab = Ab::default();
        ab.toggle(StripId::Ch(1), snap(1, 0.1, 0.2));
        ab.toggle(StripId::Ch(1), snap(1, 0.3, 0.2));
        assert_eq!(ab.active(StripId::Ch(2)), None);
        assert!(ab.toggle(StripId::Ch(2), snap(2, 0.8, 0.2)).is_empty());
        assert_eq!(ab.active(StripId::Ch(1)), Some(Slot::B));
        assert_eq!(ab.active(StripId::Ch(2)), Some(Slot::A));
        ab.clear();
        assert_eq!(ab.active(StripId::Ch(1)), None);
    }
}
