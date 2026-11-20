//! EQ frequency response for drawing (RBJ "Audio EQ Cookbook" biquads at 48 kHz).
//! An approximation of the mixer's filters — good enough to show the shape.

use std::f64::consts::PI;

const FS: f64 = 48_000.0;

/// Band types in mixer order (`/eq/N/type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandType {
    LowCut,
    LowShelf,
    Peq,
    /// "Vintage" EQ — drawn like a PEQ.
    Veq,
    HighShelf,
    HighCut,
}

impl BandType {
    pub fn from_code(c: i32) -> BandType {
        match c {
            0 => BandType::LowCut,
            1 => BandType::LowShelf,
            3 => BandType::Veq,
            4 => BandType::HighShelf,
            5 => BandType::HighCut,
            _ => BandType::Peq,
        }
    }
    /// Gain has no effect for cut filters.
    pub fn uses_gain(self) -> bool {
        !matches!(self, BandType::LowCut | BandType::HighCut)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub kind: BandType,
    pub hz: f32,
    pub gain_db: f32,
    pub q: f32,
}

/// Normalised biquad coefficients (a0 = 1).
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 3],
}

impl Biquad {
    fn new(kind: BandType, hz: f64, gain_db: f64, q: f64) -> Biquad {
        let w0 = 2.0 * PI * hz.clamp(10.0, FS * 0.49) / FS;
        let (sin, cos) = w0.sin_cos();
        let q = q.max(0.05);
        let alpha = sin / (2.0 * q);
        let a = 10f64.powf(gain_db / 40.0);
        let (b, aa) = match kind {
            BandType::Peq | BandType::Veq => (
                [1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a],
                [1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a],
            ),
            BandType::LowCut => (
                [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0],
                [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
            ),
            BandType::HighCut => (
                [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0],
                [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
            ),
            BandType::LowShelf | BandType::HighShelf => {
                let sq = 2.0 * a.sqrt() * alpha;
                let s = if kind == BandType::LowShelf { 1.0 } else { -1.0 };
                // Cookbook shelves; the high shelf is the low shelf with cos → −cos mirrored.
                (
                    [
                        a * ((a + 1.0) - s * (a - 1.0) * cos + sq),
                        s * 2.0 * a * ((a - 1.0) - s * (a + 1.0) * cos),
                        a * ((a + 1.0) - s * (a - 1.0) * cos - sq),
                    ],
                    [
                        (a + 1.0) + s * (a - 1.0) * cos + sq,
                        -s * 2.0 * ((a - 1.0) + s * (a + 1.0) * cos),
                        (a + 1.0) + s * (a - 1.0) * cos - sq,
                    ],
                )
            }
        };
        Biquad { b: [b[0] / aa[0], b[1] / aa[0], b[2] / aa[0]], a: [1.0, aa[1] / aa[0], aa[2] / aa[0]] }
    }

    /// Magnitude in dB at `hz`.
    fn db_at(&self, hz: f64) -> f64 {
        let w = 2.0 * PI * hz / FS;
        // |H(e^jw)|² via z = e^jw
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let num_re = self.b[0] + self.b[1] * c1 + self.b[2] * c2;
        let num_im = -(self.b[1] * s1 + self.b[2] * s2);
        let den_re = self.a[0] + self.a[1] * c1 + self.a[2] * c2;
        let den_im = -(self.a[1] * s1 + self.a[2] * s2);
        let mag2 = (num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im);
        10.0 * mag2.max(1e-20).log10()
    }
}

/// Response of a single band in dB at `hz`.
#[cfg(test)]
pub fn band_db(b: &Band, hz: f32) -> f32 {
    Biquad::new(b.kind, b.hz as f64, b.gain_db as f64, b.q as f64).db_at(hz as f64) as f32
}

/// Summed response (dB) of all bands plus an optional 12 dB/oct high-pass, at each frequency.
pub fn response(bands: &[Band], hpf_hz: Option<f32>, freqs: &[f32]) -> Vec<f32> {
    let mut filters: Vec<Biquad> =
        bands.iter().map(|b| Biquad::new(b.kind, b.hz as f64, b.gain_db as f64, b.q as f64)).collect();
    if let Some(h) = hpf_hz {
        filters.push(Biquad::new(BandType::LowCut, h as f64, 0.0, std::f64::consts::FRAC_1_SQRT_2));
    }
    freqs.iter().map(|&f| filters.iter().map(|q| q.db_at(f as f64)).sum::<f64>() as f32).collect()
}

/// `n` log-spaced frequencies 20 Hz … 20 kHz.
pub fn log_freqs(n: usize) -> Vec<f32> {
    (0..n).map(|i| 20.0 * 1000f32.powf(i as f32 / (n - 1) as f32)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(kind: BandType, hz: f32, g: f32, q: f32) -> Band {
        Band { kind, hz, gain_db: g, q }
    }

    #[test]
    fn peq_gain_at_centre() {
        let band = b(BandType::Peq, 1000.0, 6.0, 1.0);
        assert!((band_db(&band, 1000.0) - 6.0).abs() < 0.05);
        assert!(band_db(&band, 50.0).abs() < 0.3);
        assert!(band_db(&b(BandType::Peq, 1000.0, 0.0, 2.0), 1000.0).abs() < 1e-3);
    }

    #[test]
    fn shelves() {
        let lo = b(BandType::LowShelf, 200.0, 6.0, 0.707);
        assert!((band_db(&lo, 25.0) - 6.0).abs() < 0.3);
        assert!(band_db(&lo, 10_000.0).abs() < 0.3);
        let hi = b(BandType::HighShelf, 5000.0, -6.0, 0.707);
        assert!((band_db(&hi, 18_000.0) + 6.0).abs() < 0.6);
        assert!(band_db(&hi, 100.0).abs() < 0.3);
    }

    #[test]
    fn cuts() {
        let lc = b(BandType::LowCut, 100.0, 0.0, 0.707);
        assert!((band_db(&lc, 100.0) + 3.0).abs() < 0.2);
        assert!(band_db(&lc, 25.0) < -20.0);
        assert!(band_db(&lc, 2000.0).abs() < 0.1);
        let hc = b(BandType::HighCut, 1000.0, 0.0, 0.707);
        assert!(band_db(&hc, 8000.0) < -30.0);
    }

    #[test]
    fn response_sums() {
        let bands = [b(BandType::Peq, 1000.0, 3.0, 1.0), b(BandType::Peq, 1000.0, 3.0, 1.0)];
        let r = response(&bands, None, &[1000.0]);
        assert!((r[0] - 6.0).abs() < 0.1);
        let f = log_freqs(5);
        assert_eq!(f[0], 20.0);
        assert!((f[4] - 20_000.0).abs() < 1.0);
    }
}
