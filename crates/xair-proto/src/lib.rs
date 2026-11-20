//! X Air (XR12/16/18) OSC protocol: addresses, value conversions, meter parsing.

pub mod fader;
pub mod meters;
pub mod osc;
pub mod param;
pub mod strip;

pub use param::{Param, SendTo, Value};
pub use strip::StripId;

/// UDP port the mixer listens on.
pub const MIXER_PORT: u16 = 10024;
