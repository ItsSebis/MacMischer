//! Float 0..1 <-> real-world value scales for processing parameters.

/// Mapping between the mixer's normalised float and a physical value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scale {
    Lin { min: f32, max: f32 },
    /// `min` and `max` may be in either order (e.g. Q 10..0.3).
    Log { min: f32, max: f32 },
}

impl Scale {
    pub fn to_value(self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Scale::Lin { min, max } => min + x * (max - min),
            Scale::Log { min, max } => min * (max / min).powf(x),
        }
    }

    /// Inverse of [`Scale::to_value`], clamped to 0..1.
    pub fn from_value(self, v: f32) -> f32 {
        let x = match self {
            Scale::Lin { min, max } => (v - min) / (max - min),
            Scale::Log { min, max } => {
                if v <= 0.0 {
                    return if max > min { 0.0 } else { 1.0 };
                }
                (v / min).ln() / (max / min).ln()
            }
        };
        x.clamp(0.0, 1.0)
    }
}

/// Snap `x` (0..1) to the nearest of `steps` positions (n / (steps-1)).
pub fn steps_quantize(x: f32, steps: u32) -> f32 {
    let n = (steps.max(2) - 1) as f32;
    (x.clamp(0.0, 1.0) * n).round() / n
}

pub const FREQ: Scale = Scale::Log { min: 20.0, max: 20000.0 };
pub const HPF: Scale = Scale::Log { min: 20.0, max: 400.0 };
pub const EQ_GAIN: Scale = Scale::Lin { min: -15.0, max: 15.0 };
pub const EQ_Q: Scale = Scale::Log { min: 10.0, max: 0.3 };
pub const HOLD: Scale = Scale::Log { min: 0.02, max: 2000.0 };
pub const RELEASE: Scale = Scale::Log { min: 5.0, max: 4000.0 };
pub const ATTACK: Scale = Scale::Lin { min: 0.0, max: 120.0 };

/// "58 Hz", "496 Hz", "1.98k", "10.0k".
pub fn fmt_hz(hz: f32) -> String {
    if hz >= 10000.0 {
        format!("{:.1}k", hz / 1000.0)
    } else if hz >= 1000.0 {
        format!("{:.2}k", hz / 1000.0)
    } else {
        format!("{:.0} Hz", hz)
    }
}

/// "0.5 ms", "12 ms", "355 ms", "1.20 s".
pub fn fmt_ms(ms: f32) -> String {
    if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else if ms >= 100.0 {
        format!("{:.0} ms", ms)
    } else if ms >= 10.0 {
        format!("{:.1} ms", ms)
    } else {
        format!("{:.2} ms", ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f32, b: f32, tol: f32) {
        assert!((a - b).abs() <= tol, "{a} vs {b}");
    }

    #[test]
    fn verified_pairs() {
        near(FREQ.to_value(0.155), 58.3, 0.2);
        near(FREQ.to_value(0.465), 496.6, 1.0);
        near(FREQ.to_value(0.8), 5024.0, 10.0);
        near(FREQ.to_value(0.565), 990.9, 2.0);
        near(EQ_Q.to_value(0.45070422), 2.06, 0.01);
        near(Scale::Lin { min: -80.0, max: 0.0 }.to_value(0.25625), -59.5, 0.01);
        near(HOLD.to_value(0.85), 355.6, 1.0);
        near(RELEASE.to_value(0.76), 804.0, 2.0);
        near(RELEASE.to_value(0.51), 151.0, 1.0);
        near(HOLD.to_value(0.54), 10.0, 0.1);
        near(HPF.to_value(0.5), 89.4, 0.1);
        near(EQ_GAIN.to_value(0.75), 7.5, 1e-4);
    }

    #[test]
    fn inverse_and_steps() {
        for s in [FREQ, HPF, EQ_GAIN, EQ_Q, HOLD, RELEASE, ATTACK] {
            for i in 0..=10 {
                let x = i as f32 / 10.0;
                near(s.from_value(s.to_value(x)), x, 1e-4);
            }
        }
        assert_eq!(FREQ.from_value(1.0e9), 1.0);
        assert_eq!(FREQ.from_value(1.0), 0.0);
        near(steps_quantize(0.1551, 201), 0.155, 1e-6);
        near(steps_quantize(0.4507, 72), 32.0 / 71.0, 1e-6);
        assert_eq!(steps_quantize(2.0, 61), 1.0);
    }

    #[test]
    fn formatting() {
        assert_eq!(fmt_hz(58.3), "58 Hz");
        assert_eq!(fmt_hz(1980.0), "1.98k");
        assert_eq!(fmt_hz(10000.0), "10.0k");
        assert_eq!(fmt_ms(355.6), "356 ms");
        assert_eq!(fmt_ms(1200.0), "1.20 s");
    }
}
