//! Soft takeover: a controller is ignored until it reaches the mixer's current value.

use std::time::{Duration, Instant};
use xair_proto::fader;

// ---------------------------------------------------------------------------------------------
// Soft takeover

/// Tolerance for "controller is at the fader" (about two CC steps).
const PICKUP_TOL: f32 = 2.0 / 127.0;
/// If the mixer value moves this far from what we last sent, someone else moved it: re-arm.
const DRIFT: f32 = 0.02;
/// A "crossing" only counts if the controller moved this little between two messages ...
const CROSS_MAX_JUMP: f32 = 0.1;
/// ... or if the previous message is this recent (a fast sweep), never a stale one.
const CROSS_RECENT: Duration = Duration::from_millis(250);

/// Soft takeover (pickup): ignore the controller until it reaches or crosses the current value.
#[derive(Debug, Default, Clone)]
pub struct Pickup {
    engaged: bool,
    last_in: Option<f32>,
    last_at: Option<Instant>,
    last_out: Option<f32>,
}

impl Pickup {
    pub fn feed(&mut self, incoming: f32, current: f32) -> Option<f32> {
        self.feed_at(incoming, current, Instant::now())
    }

    /// `incoming` and `current` are positions 0..1. Returns the value to write, if engaged.
    pub fn feed_at(&mut self, incoming: f32, current: f32, now: Instant) -> Option<f32> {
        if self.engaged && self.last_out.is_some_and(|o| (current - o).abs() > DRIFT) {
            self.engaged = false;
        }
        if !self.engaged {
            let near = (incoming - current).abs() <= PICKUP_TOL;
            let crossed = self.last_in.is_some_and(|l| {
                (l - current) * (incoming - current) <= 0.0
                    && ((incoming - l).abs() <= CROSS_MAX_JUMP
                        || self.last_at.is_some_and(|t| now.saturating_duration_since(t) <= CROSS_RECENT))
            });
            self.engaged = near || crossed;
        }
        self.last_in = Some(incoming);
        self.last_at = Some(now);
        if self.engaged {
            self.last_out = Some(incoming);
            Some(incoming)
        } else {
            None
        }
    }
}

/// Largest fader position that does not exceed `db` (caps are floored, never rounded up).
fn cap_position(db: f32) -> f32 {
    (fader::from_db(db) * 1023.0).floor() / 1023.0
}

/// CC 0..127 -> fader position. Linear in fader position: 0 = -inf, 95 = -0.1 dB (unity is 0.75),
/// 127 = +10 dB; the binding's cap then limits the top end.
pub fn cc_to_fader(value: u8, max_db: Option<f32>) -> f32 {
    let pos = f32::from(value.min(127)) / 127.0;
    let cap = max_db.map_or(1.0, cap_position);
    fader::quantize(pos.min(cap))
}

/// CC 0..127 -> pan 0..1 with 64 exactly at the center.
pub fn cc_to_pan(value: u8) -> f32 {
    let v = value.min(127);
    if v <= 64 {
        f32::from(v) / 128.0
    } else {
        0.5 + f32::from(v - 64) / 126.0
    }
}

