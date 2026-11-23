//! RTA display processing shared by the EQ overlay and the RTA window: smoothing, peak hold and
//! auto gain (the visible dB window follows the signal, so a quiet source still fills the graph).

/// Fixed display window used when auto gain is off.
pub const FIXED_RANGE: (f32, f32) = (-100.0, 0.0);
/// Height of the auto-gain window in dB.
const SPAN_DB: f32 = 60.0;
/// The window top never goes below this, so silence does not zoom into the noise floor.
const MIN_TOP_DB: f32 = -30.0;
/// The window top is rounded up to this step so it does not jitter.
const STEP_DB: f32 = 6.0;
/// Headroom above the loudest band.
const HEADROOM_DB: f32 = 3.0;

/// Smoothing and decay speeds.
const RISE_TAU: f32 = 0.04; // seconds: fast attack
const FALL_TAU: f32 = 0.25; // seconds: slower release
const PEAK_FALL_DB_S: f32 = 12.0;
const GAIN_FALL_DB_S: f32 = 6.0;

#[derive(Debug, Clone, Default)]
pub struct RtaTrace {
    /// Smoothed level per band (dB).
    pub smooth: Vec<f32>,
    /// Peak-hold level per band (dB).
    pub peak: Vec<f32>,
    /// Slowly falling loudest level, the reference for the auto-gain window.
    top: f32,
}

impl RtaTrace {
    pub fn clear(&mut self) {
        self.smooth.clear();
        self.peak.clear();
        self.top = f32::NEG_INFINITY;
    }

    pub fn is_empty(&self) -> bool {
        self.smooth.is_empty()
    }

    /// Feed one frame of raw band levels (dB); `dt` is the time since the last call in seconds.
    pub fn feed(&mut self, bands: &[f32], dt: f32) {
        let dt = dt.clamp(0.0, 0.25);
        if self.smooth.len() != bands.len() {
            self.smooth = bands.to_vec();
            self.peak = bands.to_vec();
            self.top = bands.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            return;
        }
        let rise = 1.0 - (-dt / RISE_TAU).exp();
        let fall = 1.0 - (-dt / FALL_TAU).exp();
        for ((s, p), &x) in self.smooth.iter_mut().zip(self.peak.iter_mut()).zip(bands) {
            *s += (x - *s) * if x > *s { rise } else { fall };
            if *s >= *p {
                *p = *s;
            } else {
                *p = (*p - PEAK_FALL_DB_S * dt).max(*s);
            }
        }
        let loudest = self.smooth.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        self.top = if loudest >= self.top { loudest } else { (self.top - GAIN_FALL_DB_S * dt).max(loudest) };
    }

    /// Visible dB window `(bottom, top)`.
    pub fn range(&self, auto: bool) -> (f32, f32) {
        if !auto {
            return FIXED_RANGE;
        }
        let top = ((self.top + HEADROOM_DB) / STEP_DB).ceil() * STEP_DB;
        let top = if top.is_finite() { top.clamp(MIN_TOP_DB, 6.0) } else { MIN_TOP_DB };
        (top - SPAN_DB, top)
    }
}

/// A usable spectrum arrived within the last half second.
pub fn is_fresh(m: &xair_core::Meters) -> bool {
    m.rta.len() >= xair_proto::meters::RTA_BANDS
        && m.rta_updated.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(500))
}

/// 0 (bottom) .. 1 (top) position of `db` in the window.
pub fn norm(db: f32, range: (f32, f32)) -> f32 {
    ((db - range.0) / (range.1 - range.0)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(db: f32) -> Vec<f32> {
        vec![db; 100]
    }

    #[test]
    fn first_frame_is_taken_as_is() {
        let mut t = RtaTrace::default();
        t.feed(&flat(-50.0), 0.016);
        assert_eq!(t.smooth, flat(-50.0));
        assert_eq!(t.peak, flat(-50.0));
    }

    #[test]
    fn smoothing_rises_faster_than_it_falls_and_converges() {
        let mut t = RtaTrace::default();
        t.feed(&flat(-80.0), 0.016);
        t.feed(&flat(-40.0), 0.016);
        let up = t.smooth[0] + 80.0;
        let mut d = RtaTrace::default();
        d.feed(&flat(-40.0), 0.016);
        d.feed(&flat(-80.0), 0.016);
        let down = -40.0 - d.smooth[0];
        assert!(up > down, "rise {up} should beat fall {down}");
        for _ in 0..200 {
            t.feed(&flat(-40.0), 0.016);
        }
        assert!((t.smooth[0] + 40.0).abs() < 0.5);
    }

    #[test]
    fn peak_holds_above_the_trace_and_decays_slowly() {
        let mut t = RtaTrace::default();
        t.feed(&flat(-80.0), 0.016);
        for _ in 0..20 {
            t.feed(&flat(-20.0), 0.016);
        }
        let hi = t.peak[0];
        t.feed(&flat(-80.0), 0.016);
        assert!(t.peak[0] > t.smooth[0], "peak stays above the falling trace");
        assert!(t.peak[0] <= hi);
        for _ in 0..600 {
            t.feed(&flat(-80.0), 0.016);
        }
        assert!((t.peak[0] - t.smooth[0]).abs() < 0.5, "peak eventually meets the trace");
    }

    #[test]
    fn auto_gain_follows_the_signal_up_instantly_and_down_slowly() {
        let mut t = RtaTrace::default();
        t.feed(&flat(-70.0), 0.016);
        let quiet = t.range(true);
        assert_eq!(quiet.1 - quiet.0, SPAN_DB);
        assert_eq!(quiet.1, MIN_TOP_DB, "quiet signals do not zoom past the floor");
        let mut hot = flat(-70.0);
        hot[40] = -10.0;
        for _ in 0..30 {
            t.feed(&hot, 0.016);
        }
        let loud = t.range(true);
        assert!(loud.1 > quiet.1, "window moved up: {loud:?}");
        assert!(norm(t.smooth[40], loud) > 0.5);
        // Falls slowly: after one second of silence it has dropped by at most the fall rate.
        for _ in 0..60 {
            t.feed(&flat(-128.0), 0.016);
        }
        assert!(t.range(true).1 >= loud.1 - 12.0);
    }

    #[test]
    fn range_is_stepped_and_fixed_mode_is_fixed() {
        let mut t = RtaTrace::default();
        t.feed(&flat(-20.0), 0.016);
        let (lo, hi) = t.range(true);
        assert_eq!(hi % STEP_DB, 0.0);
        assert_eq!(hi - lo, SPAN_DB);
        assert_eq!(t.range(false), FIXED_RANGE);
        t.clear();
        assert!(t.is_empty());
        assert_eq!(t.range(true).1, MIN_TOP_DB);
    }

    #[test]
    fn norm_clamps() {
        assert_eq!(norm(-200.0, (-60.0, 0.0)), 0.0);
        assert_eq!(norm(10.0, (-60.0, 0.0)), 1.0);
        assert_eq!(norm(-30.0, (-60.0, 0.0)), 0.5);
    }
}
