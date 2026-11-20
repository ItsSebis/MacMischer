//! Keyboard channel selection: type digits (with optional prefix letter), auto-commit.
//!
//! `12` → Ch 12 · `a3` → Bus 3 · `f2` → FX 2 return · `s1` → FX 1 send · `d4` → DCA 4 ·
//! `r` → Aux in · `l` → Main LR · `0`/`00` → unity on the selected fader.
//! Commits on Enter, after a short pause, or as soon as no longer number is possible
//! (`2` commits immediately because there is no Ch 20+).
//!
//! In *positional* mode (per bank) plain digits pick the n-th strip of the current bank
//! instead of the mixer channel; prefixed entries (`a3`, `d1`, …) stay global.

use std::time::{Duration, Instant};
use xair_proto::strip::{BUSES, CHANNELS, DCAS, FX};
use xair_proto::StripId;

pub const TIMEOUT: Duration = Duration::from_millis(450);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prefix {
    Ch,
    Bus,
    FxRtn,
    FxSend,
    Dca,
}

impl Prefix {
    fn max(self) -> u8 {
        match self {
            Prefix::Ch => CHANNELS,
            Prefix::Bus => BUSES,
            Prefix::FxRtn | Prefix::FxSend => FX,
            Prefix::Dca => DCAS,
        }
    }
    fn strip(self, n: u8) -> StripId {
        match self {
            Prefix::Ch => StripId::Ch(n),
            Prefix::Bus => StripId::Bus(n),
            Prefix::FxRtn => StripId::FxRtn(n),
            Prefix::FxSend => StripId::FxSend(n),
            Prefix::Dca => StripId::Dca(n),
        }
    }
    fn label(self) -> &'static str {
        match self {
            Prefix::Ch => "Ch",
            Prefix::Bus => "Bus",
            Prefix::FxRtn => "FX Rtn",
            Prefix::FxSend => "FX Send",
            Prefix::Dca => "DCA",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Select(StripId),
    /// 1-based position in the current bank (positional mode).
    Position(u8),
    Unity,
}

#[derive(Debug, Default)]
pub struct NumberEntry {
    prefix: Option<Prefix>,
    digits: String,
    last: Option<Instant>,
    /// `Some(bank length)` = positional mode.
    positional: Option<u8>,
}

impl NumberEntry {
    /// Switch numbering mode (call when the bank changes). `Some(len)` = positional.
    pub fn set_positional(&mut self, len: Option<u8>) {
        self.positional = len;
    }

    fn plain_max(&self) -> u8 {
        self.positional.unwrap_or(CHANNELS)
    }

    pub fn active(&self) -> bool {
        self.prefix.is_some() || !self.digits.is_empty()
    }

    /// Text for the on-screen hint, e.g. `Bus _` or `Ch 1_`.
    pub fn display(&self) -> Option<String> {
        if !self.active() {
            return None;
        }
        if self.prefix.is_none() && self.digits.starts_with('0') {
            return Some("Unity (0 dB)".into());
        }
        let label = match (self.prefix, self.positional) {
            (Some(p), _) => p.label(),
            (None, Some(_)) => "Bank #",
            (None, None) => Prefix::Ch.label(),
        };
        Some(format!("{label} {}_", self.digits))
    }

    pub fn cancel(&mut self) {
        self.prefix = None;
        self.digits.clear();
        self.last = None;
    }

    /// Feed a typed character. Returns `None` if the character isn't ours (caller may use it).
    pub fn char(&mut self, c: char, now: Instant) -> Option<Option<Entry>> {
        let c = c.to_ascii_lowercase();
        if let Some(d) = c.to_digit(10) {
            if self.prefix.is_none() && self.digits.is_empty() && d == 0 {
                self.digits.push('0');
                self.last = Some(now);
                return Some(None);
            }
            if self.prefix.is_none() && self.digits.starts_with('0') {
                // "00" → unity right away; "0x" → treat as unity too.
                self.cancel();
                return Some(Some(Entry::Unity));
            }
            self.digits.push(char::from_digit(d, 10).unwrap());
            self.last = Some(now);
            return Some(self.try_commit_early());
        }
        let instant = match c {
            'r' => Some(StripId::AuxIn),
            'l' => Some(StripId::Main),
            _ => None,
        };
        if let Some(s) = instant {
            self.cancel();
            return Some(Some(Entry::Select(s)));
        }
        let p = match c {
            'a' => Prefix::Bus,
            'f' => Prefix::FxRtn,
            'x' => Prefix::FxSend,
            'd' => Prefix::Dca,
            _ => return None,
        };
        self.prefix = Some(p);
        self.digits.clear();
        self.last = Some(now);
        Some(None)
    }

    /// Enter pressed.
    pub fn enter(&mut self) -> Option<Entry> {
        let r = self.resolve();
        self.cancel();
        r
    }

    /// Call every frame; commits after the pause.
    pub fn poll(&mut self, now: Instant) -> Option<Entry> {
        match self.last {
            Some(t) if self.active() && now - t >= TIMEOUT => self.enter(),
            _ => None,
        }
    }

    fn resolve(&self) -> Option<Entry> {
        if self.prefix.is_none() && self.digits.starts_with('0') {
            return Some(Entry::Unity);
        }
        let n: u8 = self.digits.parse().ok()?;
        match self.prefix {
            Some(p) => (1..=p.max()).contains(&n).then(|| Entry::Select(p.strip(n))),
            None if self.positional.is_some() => (1..=self.plain_max()).contains(&n).then_some(Entry::Position(n)),
            None => (1..=CHANNELS).contains(&n).then_some(Entry::Select(StripId::Ch(n))),
        }
    }

    /// Commit now if appending another digit can't give a valid number.
    fn try_commit_early(&mut self) -> Option<Entry> {
        let max = self.prefix.map(Prefix::max).unwrap_or(self.plain_max());
        let n: u32 = self.digits.parse().ok()?;
        if n * 10 > max as u32 {
            let r = self.resolve();
            self.cancel();
            return r;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str) -> (NumberEntry, Vec<Entry>) {
        let mut e = NumberEntry::default();
        let now = Instant::now();
        let out = s.chars().filter_map(|c| e.char(c, now).flatten()).collect();
        (e, out)
    }

    #[test]
    fn single_digit_waits_when_ambiguous() {
        let (mut e, out) = typed("1");
        assert!(out.is_empty());
        assert_eq!(e.display().as_deref(), Some("Ch 1_"));
        let later = Instant::now() + TIMEOUT;
        assert_eq!(e.poll(later), Some(Entry::Select(StripId::Ch(1))));
        assert!(!e.active());
    }

    #[test]
    fn commits_early() {
        assert_eq!(typed("2").1, vec![Entry::Select(StripId::Ch(2))]);
        assert_eq!(typed("12").1, vec![Entry::Select(StripId::Ch(12))]);
        assert_eq!(typed("16").1, vec![Entry::Select(StripId::Ch(16))]);
        assert_eq!(typed("a3").1, vec![Entry::Select(StripId::Bus(3))]);
        assert_eq!(typed("f2").1, vec![Entry::Select(StripId::FxRtn(2))]);
        assert_eq!(typed("x4").1, vec![Entry::Select(StripId::FxSend(4))]);
        assert_eq!(typed("d1").1, vec![Entry::Select(StripId::Dca(1))]);
        assert_eq!(typed("r").1, vec![Entry::Select(StripId::AuxIn)]);
        assert_eq!(typed("L").1, vec![Entry::Select(StripId::Main)]);
        assert_eq!(typed("25").1, vec![Entry::Select(StripId::Ch(2)), Entry::Select(StripId::Ch(5))]);
    }

    #[test]
    fn invalid_numbers_are_dropped() {
        assert_eq!(typed("17").1, vec![]);
        assert_eq!(typed("a9").1, vec![]);
        let (e, _) = typed("17");
        assert!(!e.active());
    }

    #[test]
    fn enter_and_cancel() {
        let (mut e, _) = typed("1");
        assert_eq!(e.enter(), Some(Entry::Select(StripId::Ch(1))));
        let (mut e, _) = typed("1");
        e.cancel();
        assert_eq!(e.enter(), None);
    }

    #[test]
    fn unity() {
        assert_eq!(typed("00").1, vec![Entry::Unity]);
        let (mut e, _) = typed("0");
        assert_eq!(e.enter(), Some(Entry::Unity));
    }

    #[test]
    fn positional_mode() {
        let mut e = NumberEntry::default();
        e.set_positional(Some(8));
        let now = Instant::now();
        assert_eq!(e.char('3', now), Some(Some(Entry::Position(3)))); // 30+ impossible → instant
        assert_eq!(e.char('9', now), Some(None)); // out of range → dropped
        assert!(!e.active());
        e.set_positional(Some(12));
        assert_eq!(e.char('1', now), Some(None)); // could be 10–12
        assert_eq!(e.char('2', now), Some(Some(Entry::Position(12))));
        // prefixes stay global
        assert_eq!(e.char('a', now), Some(None));
        assert_eq!(e.char('2', now), Some(Some(Entry::Select(StripId::Bus(2)))));
    }

    #[test]
    fn foreign_chars_pass_through() {
        let mut e = NumberEntry::default();
        assert_eq!(e.char('m', Instant::now()), None);
        assert_eq!(e.char('s', Instant::now()), None);
    }
}
