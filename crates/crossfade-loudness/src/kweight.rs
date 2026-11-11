//! The K-weighting pre-filter of BS.1770-4: a high shelf then a high-pass (RLB), each a biquad.
//!
//! The standard prints coefficients for 48 kHz only. They are the bilinear transform of two
//! analogue filters, so the filters' parameters (recovered once from the published tables, as
//! the common open implementations do) are re-applied at the actual rate. At 48 kHz the result
//! equals the printed coefficients to 1e-9 (a test checks it).

use crate::error::{Error, Result};
use std::f64::consts::PI;

/// Lowest supported sample rate.
pub const MIN_SAMPLE_RATE: u32 = 8_000;
/// Highest supported sample rate (the FLAC limit).
pub const MAX_SAMPLE_RATE: u32 = 1_048_575;

const SHELF_HZ: f64 = 1681.974450955533;
const SHELF_GAIN_DB: f64 = 3.999843853973347;
const SHELF_Q: f64 = 0.7071752369554196;
// The shelf's band gain is Vh^0.4996667741545416 (the analogue prototype's geometric mean).
const SHELF_BAND_EXP: f64 = 0.4996667741545416;
const HP_HZ: f64 = 38.13547087602444;
const HP_Q: f64 = 0.5003270373238773;

/// One second-order section, `a[0]` is 1: `y = b0 x + b1 x1 + b2 x2 - a1 y1 - a2 y2`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    pub b: [f64; 3],
    pub a: [f64; 3],
}

/// The two stages for one sample rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KWeighting {
    pub sample_rate: u32,
    pub shelf: Biquad,
    pub highpass: Biquad,
}

impl KWeighting {
    /// Filters for `rate`; an error outside [`MIN_SAMPLE_RATE`]..=[`MAX_SAMPLE_RATE`].
    pub fn new(rate: u32) -> Result<Self> {
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&rate) {
            return Err(Error::SampleRate { rate });
        }
        let fs = f64::from(rate);

        let k = libm::tan(PI * SHELF_HZ / fs);
        let vh = libm::pow(10.0, SHELF_GAIN_DB / 20.0);
        let vb = libm::pow(vh, SHELF_BAND_EXP);
        let k2 = k * k;
        let a0 = 1.0 + k / SHELF_Q + k2;
        let shelf = Biquad {
            b: [
                (vh + vb * k / SHELF_Q + k2) / a0,
                2.0 * (k2 - vh) / a0,
                (vh - vb * k / SHELF_Q + k2) / a0,
            ],
            a: [1.0, 2.0 * (k2 - 1.0) / a0, (1.0 - k / SHELF_Q + k2) / a0],
        };

        let k = libm::tan(PI * HP_HZ / fs);
        let k2 = k * k;
        let a0 = 1.0 + k / HP_Q + k2;
        let highpass = Biquad {
            b: [1.0, -2.0, 1.0],
            a: [1.0, 2.0 * (k2 - 1.0) / a0, (1.0 - k / HP_Q + k2) / a0],
        };
        Ok(Self {
            sample_rate: rate,
            shelf,
            highpass,
        })
    }

    /// Combined magnitude response at `hz`, in dB.
    pub fn gain_db(&self, hz: f64) -> f64 {
        let w = 2.0 * PI * hz / f64::from(self.sample_rate);
        let power = |f: &Biquad| {
            let (c1, s1) = (libm::cos(w), libm::sin(w));
            let (c2, s2) = (libm::cos(2.0 * w), libm::sin(2.0 * w));
            let mag = |c: &[f64; 3]| {
                let re = c[0] + c[1] * c1 + c[2] * c2;
                let im = c[1] * s1 + c[2] * s2;
                re * re + im * im
            };
            mag(&f.b) / mag(&f.a)
        };
        10.0 * libm::log10(power(&self.shelf) * power(&self.highpass))
    }
}
