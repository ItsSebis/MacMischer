//! Every user intent (mouse, keyboard, later MIDI) becomes an [`Action`], applied in one place.

use xair_proto::{Param, StripId, Value};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Set(Param, Value),
    Select(StripId),
    /// Select the n-th (1-based) strip of the current bank.
    SelectPosition(u8),
    SelectBank(usize),
    /// Move the selected fader (or bus send in a monitor bank) by dB.
    Nudge(f32),
    /// Fader / send to 0 dB.
    Unity,
    PanNudge(f32),
    ToggleMute,
    ToggleSolo,
    /// Solo on/off for a strip (and its linked partner).
    SetSolo(StripId, bool),
    BankStep(i32),
    /// Select next/previous strip within the current bank.
    StripStep(i32),
}

/// Fader position after moving by `db`, always at least one mixer step in that direction.
pub fn nudge_fader(f: f32, db: f32) -> f32 {
    use xair_proto::fader;
    let cur = fader::to_db(f);
    let target = if cur < -89.9 {
        if db > 0.0 { -60.0 } else { return 0.0 }
    } else {
        cur + db
    };
    let mut q = if target < -89.9 { 0.0 } else { fader::quantize(fader::from_db(target)) };
    let step = 1.0 / 1023.0;
    if (q - f).abs() < step / 2.0 && q > 0.0 {
        q = fader::quantize(f + step * db.signum());
    }
    q.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xair_proto::fader::to_db;

    #[test]
    fn nudges() {
        let unity = 767.0 / 1023.0;
        assert!((to_db(nudge_fader(unity, 1.0)) - 1.0).abs() < 0.06);
        assert!((to_db(nudge_fader(unity, -3.0)) + 3.0).abs() < 0.06);
        assert!((to_db(nudge_fader(0.0, 1.0)) + 60.0).abs() < 0.3);
        assert_eq!(nudge_fader(0.0, -1.0), 0.0);
        assert_eq!(nudge_fader(1.0, 1.0), 1.0);
        // fine steps never get stuck in coarse regions
        let low = xair_proto::fader::quantize(xair_proto::fader::from_db(-50.0));
        assert!(nudge_fader(low, 0.1) > low);
        assert!(nudge_fader(low, -0.1) < low);
    }
}
