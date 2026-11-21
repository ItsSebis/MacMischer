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

/// Number of RTA spectrum bands in `/meters/4`.
pub const RTA_BANDS: usize = 100;

/// Parse a `/meters/4` (RTA) blob into [`RTA_BANDS`] dB values clamped to `FLOOR_DB..=0`.
///
/// UNVERIFIED layout: assumed i32 count header followed by >= 100 i16 LE (1/256 dB) values;
/// the first 100 are used and anything after them is ignored. The count header is not trusted
/// (only the actual blob length is). Returns `None` for blobs with fewer than 100 values.
pub fn parse_rta(blob: &[u8]) -> Option<Vec<f32>> {
    let data = blob.get(4..)?;
    let shorts = data.as_chunks::<2>().0;
    if shorts.len() < RTA_BANDS {
        return None;
    }
    Some(
        shorts[..RTA_BANDS]
            .iter()
            .map(|c| (i16::from_le_bytes([c[0], c[1]]) as f32 / 256.0).clamp(FLOOR_DB, 0.0))
            .collect(),
    )
}

/// Assumed centre frequency (Hz) of RTA band `i` (0..100): log-spaced 20 Hz to 20 kHz.
/// UNVERIFIED: the mixer's real band layout has not been checked against hardware.
pub fn rta_band_hz(i: usize) -> f32 {
    20.0 * 1000f32.powf(i.min(RTA_BANDS - 1) as f32 / (RTA_BANDS - 1) as f32)
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

    fn rta_blob(vals: &[i16], count: i32) -> Vec<u8> {
        let mut v = count.to_le_bytes().to_vec();
        for x in vals {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v
    }

    #[test]
    fn rta_valid() {
        let vals: Vec<i16> = (0..100).map(|i| -(i as i16) * 256).collect();
        let r = parse_rta(&rta_blob(&vals, 100)).unwrap();
        assert_eq!(r.len(), 100);
        assert_eq!(r[0], 0.0);
        assert_eq!(r[10], -10.0);
    }

    #[test]
    fn rta_extra_values_and_bad_count_header() {
        let vals = vec![-256i16; 106];
        assert_eq!(parse_rta(&rta_blob(&vals, 106)).unwrap().len(), 100);
        assert_eq!(parse_rta(&rta_blob(&vals, -5)).unwrap().len(), 100);
        assert_eq!(parse_rta(&rta_blob(&vals, i32::MAX)).unwrap().len(), 100);
    }

    #[test]
    fn rta_short_or_odd() {
        assert_eq!(parse_rta(&[]), None);
        assert_eq!(parse_rta(&[1, 2]), None);
        assert_eq!(parse_rta(&rta_blob(&[0; 99], 99)), None);
        let mut odd = rta_blob(&[0; 100], 100);
        odd.push(7);
        assert_eq!(parse_rta(&odd).unwrap().len(), 100);
        let mut odd_short = rta_blob(&[0; 99], 99);
        odd_short.push(7);
        assert_eq!(parse_rta(&odd_short), None);
    }

    #[test]
    fn rta_extremes_clamped() {
        let mut vals = vec![0i16; 100];
        vals[0] = i16::MAX;
        vals[1] = i16::MIN;
        vals[2] = 256;
        let r = parse_rta(&rta_blob(&vals, 100)).unwrap();
        assert_eq!(r[0], 0.0);
        assert_eq!(r[1], FLOOR_DB);
        assert_eq!(r[2], 0.0);
    }

    #[test]
    fn rta_band_centres() {
        assert!((rta_band_hz(0) - 20.0).abs() < 1e-3);
        assert!((rta_band_hz(99) - 20_000.0).abs() < 1.0);
        assert!(rta_band_hz(50) > rta_band_hz(49));
        assert!((rta_band_hz(1000) - 20_000.0).abs() < 1.0);
    }

    #[test]
    fn malformed() {
        assert_eq!(parse(&[1, 0]), None);
        assert_eq!(parse(&[5, 0, 0, 0, 1, 2]), None);
    }
}
