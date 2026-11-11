//! True peak (BS.1770-4 Annex 2): the largest magnitude of the oversampled signal.
//!
//! The standard's interpolator is a 4x polyphase FIR (2x from 88.2 kHz, none from 176.4 kHz).
//! This one is our own: a Kaiser-windowed sinc, [`TAPS`] taps per phase, built at construction
//! from `libm` and plain arithmetic, so it is the same on every platform. Each phase is
//! normalised to unit DC gain. Phase 0 is the input sample itself (the sample peak), so the
//! true peak is never below the sample peak.

/// Taps per phase; even, so the interpolation point sits between two middle taps.
pub(crate) const TAPS: usize = 12;
/// Kaiser beta: trades passband flatness near Nyquist against stopband depth.
const BETA: f64 = 6.0;
/// Oversampled rates should reach at least this (192 kHz is the 4x of 48 kHz).
const TARGET_RATE: u32 = 176_400;

/// Oversampling factor for a rate: 4 below 88.2 kHz, 2 below 176.4 kHz, else 1.
pub(crate) fn factor_for(rate: u32) -> usize {
    if rate >= TARGET_RATE {
        1
    } else if rate >= TARGET_RATE / 2 {
        2
    } else {
        4
    }
}

/// Zeroth-order modified Bessel function, by its power series.
fn bessel_i0(x: f64) -> f64 {
    let q = x * x / 4.0;
    let (mut term, mut sum) = (1.0, 1.0);
    let mut k = 1.0;
    while term > sum * 1e-17 {
        term *= q / (k * k);
        sum += term;
        k += 1.0;
    }
    sum
}

#[derive(Debug, Clone)]
pub(crate) struct Interpolator {
    /// Taps of phases 1..factor, oldest input first.
    phases: Vec<[f64; TAPS]>,
}

impl Interpolator {
    pub(crate) fn new(factor: usize) -> Self {
        let half = (TAPS / 2) as f64;
        let norm = bessel_i0(BETA);
        let mut phases = Vec::new();
        for p in 1..factor {
            let mut taps = [0.0; TAPS];
            for (j, t) in taps.iter_mut().enumerate() {
                // Distance from the interpolation point to input j, in input samples.
                let tau = p as f64 / factor as f64 + half - 1.0 - j as f64;
                let x = core::f64::consts::PI * tau;
                let sinc = libm::sin(x) / x;
                let r = tau / half;
                let window = bessel_i0(BETA * libm::sqrt(1.0 - r * r)) / norm;
                *t = sinc * window;
            }
            let sum: f64 = taps.iter().sum();
            for t in &mut taps {
                *t /= sum;
            }
            phases.push(taps);
        }
        Self { phases }
    }
}

/// The last [`TAPS`] inputs of one channel, in a doubled ring so the window is contiguous.
#[derive(Debug, Clone)]
pub(crate) struct History {
    buf: [f64; 2 * TAPS],
    pos: usize,
}

impl History {
    pub(crate) fn new() -> Self {
        Self {
            buf: [0.0; 2 * TAPS],
            pos: 0,
        }
    }

    /// Takes one input and returns the largest magnitude among the in-between points it
    /// completes (the output is delayed by half the filter).
    #[inline]
    pub(crate) fn push(&mut self, interp: &Interpolator, x: f64) -> f64 {
        self.buf[self.pos] = x;
        self.buf[self.pos + TAPS] = x;
        self.pos = (self.pos + 1) % TAPS;
        let window = &self.buf[self.pos..self.pos + TAPS];
        let mut peak = 0.0f64;
        for taps in &interp.phases {
            let mut y = 0.0;
            for (t, v) in taps.iter().zip(window) {
                y += t * v;
            }
            peak = peak.max(y.abs());
        }
        peak
    }

    /// The peak the filter would still add if the signal stopped here (zeros follow).
    pub(crate) fn tail_peak(&self, interp: &Interpolator) -> f64 {
        let mut copy = self.clone();
        let mut peak = 0.0f64;
        for _ in 0..TAPS / 2 {
            peak = peak.max(copy.push(interp, 0.0));
        }
        peak
    }
}
