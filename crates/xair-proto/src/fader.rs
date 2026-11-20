//! Fader law: mixer float `0.0..=1.0` <-> dB (X32/X Air family, 4 linear segments).
//! `0.0` is -inf; `1.0` is +10 dB; `0.75` is 0 dB (unity).

/// Level in dB; `f32::NEG_INFINITY` means fully off.
pub fn to_db(f: f32) -> f32 {
    let f = f.clamp(0.0, 1.0);
    if f <= 0.0 {
        f32::NEG_INFINITY
    } else if f >= 0.5 {
        f * 40.0 - 30.0
    } else if f >= 0.25 {
        f * 80.0 - 50.0
    } else if f >= 0.0625 {
        f * 160.0 - 70.0
    } else {
        f * 480.0 - 90.0
    }
}

/// Inverse of [`to_db`]. Values below -90 dB map to 0.0, above +10 dB to 1.0.
pub fn from_db(db: f32) -> f32 {
    let f = if db <= -90.0 {
        0.0
    } else if db >= -10.0 {
        (db + 30.0) / 40.0
    } else if db >= -30.0 {
        (db + 50.0) / 80.0
    } else if db >= -60.0 {
        (db + 70.0) / 160.0
    } else {
        (db + 90.0) / 480.0
    };
    f.clamp(0.0, 1.0)
}

/// Mixer fader steps are 1024 positions; snap so we send what the mixer stores.
pub fn quantize(f: f32) -> f32 {
    (f.clamp(0.0, 1.0) * 1023.0).round() / 1023.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_points() {
        assert_eq!(to_db(0.0), f32::NEG_INFINITY);
        assert!((to_db(0.75) - 0.0).abs() < 1e-5);
        assert!((to_db(1.0) - 10.0).abs() < 1e-5);
        assert!((to_db(0.5) - -10.0).abs() < 1e-5);
        assert!((to_db(0.25) - -30.0).abs() < 1e-5);
        assert!((to_db(0.0625) - -60.0).abs() < 1e-5);
    }

    #[test]
    fn round_trip() {
        for i in 1..=1000 {
            let f = i as f32 / 1000.0;
            assert!((from_db(to_db(f)) - f).abs() < 1e-4, "f={f}");
        }
    }

    #[test]
    fn clamps() {
        assert_eq!(from_db(f32::NEG_INFINITY), 0.0);
        assert_eq!(from_db(20.0), 1.0);
    }
}
