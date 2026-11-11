//! Filter design: a windowed-sinc low-pass and the polyphase tables sampled from it.
//!
//! Only `+ - * /`, `sqrt` and `libm` are used here, so a table is the same bits on every
//! platform, and so is everything computed from it.

use crate::quality::Params;
use std::f64::consts::PI;

/// Taps are a multiple of this so the kernels need no remainder loop.
pub(crate) const TAP_MULTIPLE: usize = 16;

/// A zero-ended Kaiser-windowed sinc, in units of the sample spacing it is evaluated at.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Kernel {
    /// Cutoff as a fraction of the evaluation rate's Nyquist frequency.
    cutoff: f64,
    /// The window reaches zero at `+-half`.
    half: f64,
    beta: f64,
    /// `I0(beta) - 1`, the window's normaliser.
    norm: f64,
}

impl Kernel {
    /// `scale` is the low rate over the evaluation rate (1 when evaluated at the low rate);
    /// `half` is the window's half-width at the evaluation rate.
    pub(crate) fn new(params: &Params, scale: f64, half: f64) -> Self {
        let beta = params.beta();
        Self {
            cutoff: scale * params.cutoff(),
            half,
            beta,
            norm: bessel_i0(beta) - 1.0,
        }
    }

    pub(crate) fn at(&self, v: f64) -> f64 {
        let x = v / self.half;
        if x.abs() >= 1.0 {
            return 0.0;
        }
        // Minus the pedestal, so the kernel is continuous where it meets zero; interpolating
        // between table rows then stays accurate up to the edge.
        let window = (bessel_i0(self.beta * (1.0 - x * x).sqrt()) - 1.0) / self.norm;
        self.cutoff * sinc(self.cutoff * v) * window
    }
}

/// `sin(pi x) / (pi x)`. The argument is reduced exactly into [-1/2, 1/2] first (whole
/// periods off, then `sin(pi r) = sin(pi (1 - r))`), so zeros land exactly on the integers.
fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    let mut r = x - 2.0 * libm::round(x / 2.0);
    if r > 0.5 {
        r = 1.0 - r;
    } else if r < -0.5 {
        r = -1.0 - r;
    }
    libm::sin(PI * r) / (PI * x)
}

/// The modified Bessel function of order zero, by its power series (all terms positive).
pub(crate) fn bessel_i0(x: f64) -> f64 {
    let q = x * x / 4.0;
    let (mut sum, mut term) = (1.0, 1.0);
    let mut k = 1.0;
    while term > sum * 1e-18 && k < 500.0 {
        term = term * q / (k * k);
        sum += term;
        k += 1.0;
    }
    sum
}

/// Taps per output at the evaluation rate: the design width at the low rate, stretched by
/// `1 / scale`, rounded up to [`TAP_MULTIPLE`].
pub(crate) fn taps(params: &Params, scale: f64) -> usize {
    let n = libm::ceil(params.width() / scale) as usize;
    n.div_ceil(TAP_MULTIPLE) * TAP_MULTIPLE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bessel_i0_matches_known_values() {
        assert_eq!(bessel_i0(0.0), 1.0);
        assert!((bessel_i0(1.0) - 1.266_065_877_752_008_4).abs() < 1e-15);
        assert!((bessel_i0(10.0) / 2_815.716_628_466_254 - 1.0).abs() < 1e-14);
    }

    #[test]
    fn sinc_is_exactly_zero_at_nonzero_integers() {
        assert_eq!(sinc(0.0), 1.0);
        for k in 1..50 {
            assert_eq!(sinc(f64::from(k)), 0.0);
            assert_eq!(sinc(-f64::from(k)), 0.0);
        }
        assert!((sinc(0.5) - 2.0 / PI).abs() < 1e-16);
    }
}
