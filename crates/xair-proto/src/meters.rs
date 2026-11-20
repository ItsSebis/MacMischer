//! Meter blobs: i32 LE count, then count × i16 LE in 1/256 dB.

/// Silence floor reported by the mixer.
pub const FLOOR_DB: f32 = -128.0;

/// `/meters/1` layout (40 values). Grouping per docs/PROTOCOL.md (to confirm with signal).
pub mod m1 {
    pub const CH: usize = 0; // 16
    pub const AUX: usize = 16; // 2 (L/R)
    pub const FX_RTN: usize = 18; // 4 × L/R
    pub const BUS: usize = 26; // 6
    pub const FX_SEND: usize = 32; // 4
    pub const MAIN: usize = 36; // 2 (L/R)
    pub const MON: usize = 38; // 2
    pub const LEN: usize = 40;
}

/// Parse a meter blob into dB values. Returns `None` if malformed.
pub fn parse(blob: &[u8]) -> Option<Vec<f32>> {
    let count = i32::from_le_bytes(blob.get(0..4)?.try_into().ok()?);
    let count = usize::try_from(count).ok()?;
    let data = blob.get(4..4 + count * 2)?;
    Some(
        data.as_chunks::<2>().0.iter()
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 256.0)
            .collect(),
    )
}

/// Build a blob (used by the mock mixer and tests).
pub fn encode(db: &[f32]) -> Vec<u8> {
    let mut v = (db.len() as i32).to_le_bytes().to_vec();
    for d in db {
        let raw = (d.clamp(FLOOR_DB, 127.0) * 256.0) as i16;
        v.extend_from_slice(&raw.to_le_bytes());
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let vals = [-128.0, -60.5, 0.0, -3.25];
        assert_eq!(parse(&encode(&vals)).unwrap(), vals);
    }

    #[test]
    fn malformed() {
        assert_eq!(parse(&[1, 0]), None);
        assert_eq!(parse(&[5, 0, 0, 0, 1, 2]), None);
    }
}
