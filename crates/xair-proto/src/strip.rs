//! Mixer strips (anything with a fader) and their OSC address prefixes.

use std::fmt;

pub const CHANNELS: u8 = 16;
pub const BUSES: u8 = 6;
pub const FX: u8 = 4;
pub const DCAS: u8 = 4;
pub const MUTE_GROUPS: u8 = 4;

/// A strip with a fader. Numbers are 1-based like on the mixer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StripId {
    Ch(u8),
    AuxIn,
    FxRtn(u8),
    Bus(u8),
    FxSend(u8),
    Dca(u8),
    Main,
}

impl StripId {
    /// Every strip on an XR18, in console order.
    pub fn all() -> Vec<StripId> {
        let mut v: Vec<StripId> = (1..=CHANNELS).map(StripId::Ch).collect();
        v.push(StripId::AuxIn);
        v.extend((1..=FX).map(StripId::FxRtn));
        v.extend((1..=BUSES).map(StripId::Bus));
        v.extend((1..=FX).map(StripId::FxSend));
        v.extend((1..=DCAS).map(StripId::Dca));
        v.push(StripId::Main);
        v
    }

    pub fn is_valid(self) -> bool {
        match self {
            StripId::Ch(n) => (1..=CHANNELS).contains(&n),
            StripId::FxRtn(n) | StripId::FxSend(n) => (1..=FX).contains(&n),
            StripId::Bus(n) => (1..=BUSES).contains(&n),
            StripId::Dca(n) => (1..=DCAS).contains(&n),
            StripId::AuxIn | StripId::Main => true,
        }
    }

    /// OSC prefix, e.g. `/ch/01`, `/rtn/aux`, `/lr`.
    pub fn prefix(self) -> String {
        match self {
            StripId::Ch(n) => format!("/ch/{n:02}"),
            StripId::AuxIn => "/rtn/aux".into(),
            StripId::FxRtn(n) => format!("/rtn/{n}"),
            StripId::Bus(n) => format!("/bus/{n}"),
            StripId::FxSend(n) => format!("/fxsend/{n}"),
            StripId::Dca(n) => format!("/dca/{n}"),
            StripId::Main => "/lr".into(),
        }
    }

    /// Parse a prefix; returns the strip and the remaining path (starting with `/` or empty).
    pub fn parse_prefix(path: &str) -> Option<(StripId, &str)> {
        let mut it = path.splitn(4, '/');
        if !it.next()?.is_empty() {
            return None;
        }
        let kind = it.next()?;
        if kind == "lr" {
            return Some((StripId::Main, rest_after(path, 1)));
        }
        let idx = it.next()?;
        let strip = match (kind, idx) {
            ("rtn", "aux") => StripId::AuxIn,
            ("ch", n) => StripId::Ch(n.parse().ok()?),
            ("rtn", n) => StripId::FxRtn(n.parse().ok()?),
            ("bus", n) => StripId::Bus(n.parse().ok()?),
            ("fxsend", n) => StripId::FxSend(n.parse().ok()?),
            ("dca", n) => StripId::Dca(n.parse().ok()?),
            _ => return None,
        };
        strip.is_valid().then(|| (strip, rest_after(path, 2)))
    }

    /// Strips that can send to buses / FX (have `mix/NN/level`).
    pub fn has_sends(self) -> bool {
        matches!(self, StripId::Ch(_) | StripId::AuxIn | StripId::FxRtn(_))
    }

    /// Strips that can be assigned to DCAs and mute groups (`grp/dca`, `grp/mute`).
    pub fn has_groups(self) -> bool {
        !matches!(self, StripId::Dca(_) | StripId::Main)
    }

    pub fn has_pan(self) -> bool {
        !matches!(self, StripId::Dca(_) | StripId::FxSend(_))
    }

    /// Head-amp index (1-based) feeding this strip, if fixed: ch N → N. (Aux in = 17/18.)
    pub fn headamp(self) -> Option<u8> {
        match self {
            StripId::Ch(n) => Some(n),
            StripId::AuxIn => Some(17),
            _ => None,
        }
    }

    /// Index for `/-stat/solosw/NN`. UNVERIFIED mapping (X Air docs): ch 1–16, aux 17,
    /// FX rtn 18–21, bus 22–27, FX send 28–31, LR 32, DCA 33–36.
    pub fn solo_index(self) -> u8 {
        match self {
            StripId::Ch(n) => n,
            StripId::AuxIn => 17,
            StripId::FxRtn(n) => 17 + n,
            StripId::Bus(n) => 21 + n,
            StripId::FxSend(n) => 27 + n,
            StripId::Main => 32,
            StripId::Dca(n) => 32 + n,
        }
    }

    /// Inverse of [`StripId::solo_index`].
    pub fn from_solo_index(n: u8) -> Option<StripId> {
        StripId::all().into_iter().find(|s| s.solo_index() == n)
    }

    /// Short label like on the console: `12`, `AUX`, `FX2`, `B3`, `S1`, `D4`, `LR`.
    pub fn short(self) -> String {
        match self {
            StripId::Ch(n) => format!("{n}"),
            StripId::AuxIn => "AUX".into(),
            StripId::FxRtn(n) => format!("FX{n}"),
            StripId::Bus(n) => format!("B{n}"),
            StripId::FxSend(n) => format!("S{n}"),
            StripId::Dca(n) => format!("D{n}"),
            StripId::Main => "LR".into(),
        }
    }

    /// Inverse of [`StripId::short`]; case-insensitive, `None` if unknown or out of range.
    pub fn from_short(s: &str) -> Option<StripId> {
        let s = s.trim().to_ascii_uppercase();
        let num = |t: &str| {
            if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) { None } else { t.parse::<u8>().ok() }
        };
        let id = match s.as_str() {
            "AUX" => StripId::AuxIn,
            "LR" => StripId::Main,
            _ if s.starts_with("FX") => StripId::FxRtn(num(&s[2..])?),
            _ if s.starts_with('B') => StripId::Bus(num(&s[1..])?),
            _ if s.starts_with('S') => StripId::FxSend(num(&s[1..])?),
            _ if s.starts_with('D') => StripId::Dca(num(&s[1..])?),
            _ => StripId::Ch(num(&s)?),
        };
        id.is_valid().then_some(id)
    }
}

/// Path after the first `segments` segments of `path` (keeps the leading `/`).
fn rest_after(path: &str, segments: usize) -> &str {
    let mut pos = 0;
    for _ in 0..segments {
        match path[pos + 1..].find('/') {
            Some(i) => pos += 1 + i,
            None => return "",
        }
    }
    &path[pos..]
}

impl fmt::Display for StripId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StripId::Ch(n) => write!(f, "Ch {n}"),
            StripId::AuxIn => write!(f, "Aux In"),
            StripId::FxRtn(n) => write!(f, "FX {n} Rtn"),
            StripId::Bus(n) => write!(f, "Bus {n}"),
            StripId::FxSend(n) => write!(f, "FX {n} Send"),
            StripId::Dca(n) => write!(f, "DCA {n}"),
            StripId::Main => write!(f, "Main LR"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_round_trip() {
        for s in StripId::all() {
            assert_eq!(StripId::from_short(&s.short()), Some(s));
            assert_eq!(StripId::from_short(&s.short().to_lowercase()), Some(s));
        }
        for bad in ["", "0", "17", "FX5", "B7", "S5", "D5", "x", "B", "+3"] {
            assert_eq!(StripId::from_short(bad), None, "{bad}");
        }
    }

    #[test]
    fn prefix_round_trip() {
        for s in StripId::all() {
            let p = s.prefix();
            let (back, rest) = StripId::parse_prefix(&p).unwrap();
            assert_eq!(back, s);
            assert_eq!(rest, "");
            let long = format!("{p}/mix/fader");
            assert_eq!(StripId::parse_prefix(&long), Some((s, "/mix/fader")));
        }
    }

    #[test]
    fn rejects_invalid() {
        assert_eq!(StripId::parse_prefix("/ch/17/mix/fader"), None);
        assert_eq!(StripId::parse_prefix("/bus/7/mix/fader"), None);
        assert_eq!(StripId::parse_prefix("/foo/1"), None);
        assert_eq!(StripId::all().len(), 16 + 1 + 4 + 6 + 4 + 4 + 1);
    }
}
