//! Output routing (`/routing/...`) and channel input source helpers.

/// A physical/virtual output whose source (and, for most, tap point) is selectable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Output {
    /// Main outputs 1..2 (`/routing/main/NN/src`, no tap). 1 = LR, 2 = MON by default.
    Main(u8),
    /// Aux outputs 1..6.
    Aux(u8),
    /// USB sends to the computer 1..18.
    Usb(u8),
    /// Ultranet P16 outputs 1..16.
    P16(u8),
}

pub const MAIN_OUTS: u8 = 2;
pub const AUX_OUTS: u8 = 6;
pub const USB_OUTS: u8 = 18;
pub const P16_OUTS: u8 = 16;

/// Number of known source codes (0..=55).
pub const SOURCE_COUNT: i32 = 56;
/// Number of known tap codes (0..=10).
pub const TAP_COUNT: i32 = 11;

impl Output {
    pub fn count(self) -> u8 {
        Self::kind_count(self)
    }
    fn kind_count(self) -> u8 {
        match self {
            Output::Main(_) => MAIN_OUTS,
            Output::Aux(_) => AUX_OUTS,
            Output::Usb(_) => USB_OUTS,
            Output::P16(_) => P16_OUTS,
        }
    }
    pub fn number(self) -> u8 {
        match self {
            Output::Main(n) | Output::Aux(n) | Output::Usb(n) | Output::P16(n) => n,
        }
    }
    pub fn is_valid(self) -> bool {
        (1..=self.kind_count()).contains(&self.number())
    }
    /// Address prefix, e.g. `/routing/aux/01`.
    pub fn prefix(self) -> String {
        let k = match self {
            Output::Main(_) => "main",
            Output::Aux(_) => "aux",
            Output::Usb(_) => "usb",
            Output::P16(_) => "p16",
        };
        format!("/routing/{k}/{:02}", self.number())
    }
    /// Whether the output has a tap-point (`/pos`) sub-address.
    pub fn has_pos(self) -> bool {
        !matches!(self, Output::Main(_))
    }
    /// All outputs of the same kind as `self`.
    pub fn all_of_kind(self) -> Vec<Output> {
        let mk: fn(u8) -> Output = match self {
            Output::Main(_) => Output::Main,
            Output::Aux(_) => Output::Aux,
            Output::Usb(_) => Output::Usb,
            Output::P16(_) => Output::P16,
        };
        (1..=self.kind_count()).map(mk).collect()
    }
    pub fn label(self) -> String {
        match self {
            Output::Main(n) => format!("Main out {n}"),
            Output::Aux(n) => format!("Aux out {n}"),
            Output::Usb(n) => format!("USB {n}"),
            Output::P16(n) => format!("Ultranet {n}"),
        }
    }
    /// Parse `/routing/<kind>/NN` into an output (no validation of the remainder).
    pub fn parse_prefix(kind: &str, n: &str) -> Option<Output> {
        let n: u8 = n.parse().ok()?;
        let o = match kind {
            "main" => Output::Main(n),
            "aux" => Output::Aux(n),
            "usb" => Output::Usb(n),
            "p16" => Output::P16(n),
            _ => return None,
        };
        o.is_valid().then_some(o)
    }
}

/// Label for an output source code. 0..=17 and 26..=31 hardware-verified; the rest inferred.
/// 0–15 Ch01–Ch16, 16/17 AuxL/AuxR, 18–25 FX1L..FX4R (inferred), 26–31 Bus1–Bus6,
/// 32–35 Send1–Send4 (inferred), 36/37 L/R (inferred), 38–55 U01–U18 (inferred).
pub fn source_label(code: i32) -> String {
    match code {
        0..=15 => format!("Ch{:02}", code + 1),
        16 => "AuxL".into(),
        17 => "AuxR".into(),
        18..=25 => format!("FX{}{}", (code - 18) / 2 + 1, if code % 2 == 0 { "L" } else { "R" }),
        26..=31 => format!("Bus{}", code - 25),
        32..=35 => format!("Send{}", code - 31),
        36 => "L".into(),
        37 => "R".into(),
        38..=55 => format!("U{:02}", code - 37),
        _ => format!("#{code}"),
    }
}

/// Label for a tap ("pos") code. 0, 3, 10 hardware-verified; the rest inferred.
pub fn tap_label(code: i32) -> &'static str {
    match code {
        0 => "Analog in",
        1 => "Analog in +M",
        2 => "Input",
        3 => "Input +M",
        4 => "Pre EQ",
        5 => "Pre EQ +M",
        6 => "Post EQ",
        7 => "Post EQ +M",
        8 => "Pre fader",
        9 => "Pre fader +M",
        10 => "Post fader",
        _ => "?",
    }
}

/// Analog input label for `/ch/NN/config/insrc` value 0..15.
pub fn analog_in_label(i: i32) -> String {
    format!("In{:02}", i + 1)
}

/// USB return label for `/ch/NN/config/rtnsrc` value 0..17.
pub fn usb_return_label(i: i32) -> String {
    format!("U{:02}", i + 1)
}

/// USB pair label for `/rtn/aux/config/rtnsrc` value 0..8 ("U01-02" .. "U17-18").
pub fn aux_usb_pair_label(i: i32) -> String {
    format!("U{:02}-{:02}", 2 * i + 1, 2 * i + 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_and_labels() {
        assert_eq!(Output::Aux(1).prefix(), "/routing/aux/01");
        assert_eq!(Output::P16(16).prefix(), "/routing/p16/16");
        assert!(!Output::Main(1).has_pos());
        assert!(Output::Usb(3).has_pos());
        assert_eq!(Output::Usb(3).label(), "USB 3");
        assert_eq!(Output::Main(2).label(), "Main out 2");
        assert_eq!(Output::Usb(1).all_of_kind().len(), 18);
        assert_eq!(Output::Aux(1).all_of_kind().last(), Some(&Output::Aux(6)));
        assert_eq!(Output::parse_prefix("aux", "07"), None);
        assert_eq!(Output::parse_prefix("p16", "05"), Some(Output::P16(5)));
    }

    #[test]
    fn source_labels() {
        assert_eq!(source_label(0), "Ch01");
        assert_eq!(source_label(15), "Ch16");
        assert_eq!(source_label(17), "AuxR");
        assert_eq!(source_label(18), "FX1L");
        assert_eq!(source_label(25), "FX4R");
        assert_eq!(source_label(26), "Bus1");
        assert_eq!(source_label(31), "Bus6");
        assert_eq!(source_label(32), "Send1");
        assert_eq!(source_label(36), "L");
        assert_eq!(source_label(38), "U01");
        assert_eq!(source_label(55), "U18");
        assert_eq!(source_label(56), "#56");
        assert_eq!(source_label(-1), "#-1");
    }

    #[test]
    fn taps_and_inputs() {
        assert_eq!(tap_label(0), "Analog in");
        assert_eq!(tap_label(3), "Input +M");
        assert_eq!(tap_label(10), "Post fader");
        assert_eq!(analog_in_label(0), "In01");
        assert_eq!(usb_return_label(1), "U02");
        assert_eq!(aux_usb_pair_label(8), "U17-18");
    }
}
