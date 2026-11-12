//! ALAC's adaptive FIR predictor, decoding side. Each sample is predicted from the `n` before
//! it, relative to the one before those, and after each sample the coefficients step by the
//! sign of how each tap moved, toward a smaller residual. The arithmetic is 32-bit and wraps,
//! as in the reference decoder; coefficients are 16-bit and wrap too.

use crate::bits::sign_extend;

/// The order that means "first difference, no coefficients".
pub(crate) const FIRST_DIFFERENCE: usize = 31;

/// Rebuilds `out` from `residual` (the same length). `coefs.len()` is the order: 0 copies, 31
/// integrates once, anything else runs the adaptive filter, adapting `coefs` as it goes.
/// Samples wrap to `chan_bits` bits.
pub(crate) fn unpredict(
    residual: &[i32],
    out: &mut [i32],
    coefs: &mut [i16],
    chan_bits: u32,
    den_shift: u32,
) {
        let _ = (residual, out, coefs, chan_bits, den_shift);
    }

/// In place: each sample becomes itself plus the one before it, wrapped to `chan_bits`.
pub(crate) fn integrate(buf: &mut [i32], chan_bits: u32) {
        let _ = (buf, chan_bits);
    }

#[inline(always)]
fn filter(
    residual: &[i32],
    out: &mut [i32],
    coefs: &mut [i16],
    order: usize,
    chan_bits: u32,
    den_shift: u32,
) {
    let coefs = &mut coefs[..order];
    // The reference takes `1 << (den_shift - 1)`, undefined for a shift of 0; no rounding then.
    let den_half = if den_shift == 0 {
        0
    } else {
        1i32 << (den_shift - 1)
    };
    let span = order + 1;
    for j in span..out.len() {
        // window[0] is the base, window[1..] the taps, oldest first.
        let (window, rest) = out[j - span..].split_at_mut(span);
        let top = window[0];
        let taps = &window[1..];
        let mut sum = 0i32;
        for (k, c) in coefs.iter().enumerate() {
            let d = taps[order - 1 - k].wrapping_sub(top);
            sum = sum.wrapping_add(i32::from(*c).wrapping_mul(d));
        }
        let del = residual[j];
        let predicted = sum.wrapping_add(den_half) >> den_shift;
        rest[0] = sign_extend(del.wrapping_add(top).wrapping_add(predicted), chan_bits);
        let mut left = del;
        if del > 0 {
            for k in (0..order).rev() {
                let dd = top.wrapping_sub(taps[order - 1 - k]);
                let sgn = dd.signum();
                coefs[k] = coefs[k].wrapping_sub(sgn as i16);
                let weight = (order - k) as i32;
                left = left.wrapping_sub(weight.wrapping_mul(sgn.wrapping_mul(dd) >> den_shift));
                if left <= 0 {
                    break;
                }
            }
        } else if del < 0 {
            for k in (0..order).rev() {
                let dd = top.wrapping_sub(taps[order - 1 - k]);
                let sgn = dd.signum();
                coefs[k] = coefs[k].wrapping_add(sgn as i16);
                let weight = (order - k) as i32;
                left = left.wrapping_sub(
                    weight.wrapping_mul(sgn.wrapping_neg().wrapping_mul(dd) >> den_shift),
                );
                if left >= 0 {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::predict;
    use proptest::prelude::*;

    #[test]
    fn order_zero_copies_and_31_integrates() {
        let r = [5, -2, 3, 0, 1];
        let mut out = [0; 5];
        unpredict(&r, &mut out, &mut [], 16, 9);
        assert_eq!(out, r);
        unpredict(&r, &mut out, &mut [0; 31], 16, 9);
        assert_eq!(out, [5, 3, 6, 6, 7]);
        // Wrapping at the channel's width: 4-bit samples run 7 -> -8.
        let mut b = [7, 1, 1];
        integrate(&mut b, 4);
        assert_eq!(b, [7, -8, -7]);
        integrate(&mut [], 16);
    }

    #[test]
    fn one_tap_worked_by_hand() {
        // Order 1, coefficient 256, den_shift 9: the prediction is half the last step, rounded.
        // out[0] = 10; out[1] = 10 + 20 = 30 (warm-up: residual plus the previous sample).
        // j = 2: top = 10, tap 30, sum = 256 * 20 = 5120, (5120 + 256) >> 9 = 10;
        //   out = 4 + 10 + 10 = 24. The residual 4 > 0: dd = 10 - 30 = -20, coefficient
        //   256 + 1 = 257.
        // j = 3: top = 30, tap 24, sum = 257 * -6 = -1542, (-1542 + 256) >> 9 = -3;
        //   out = -1 + 30 - 3 = 26. Residual < 0: dd = 6, coefficient 257 + 1 = 258.
        let mut coefs = [256];
        let mut out = [0; 4];
        unpredict(&[10, 20, 4, -1], &mut out, &mut coefs, 16, 9);
        assert_eq!(out, [10, 30, 24, 26]);
        assert_eq!(coefs, [258]);
    }

    #[test]
    fn short_blocks_and_a_zero_shift() {
        let mut out = [0; 2];
        unpredict(&[3, 4], &mut out, &mut [1, 2, 3, 4], 16, 9);
        assert_eq!(out, [3, 7]);
        let mut out = [0; 4];
        unpredict(&[1, 1, 1, 1], &mut out, &mut [1], 16, 0);
        // j = 2: top 1, tap 2, sum 1, no rounding: 1 + 1 + 1 = 3.
        assert_eq!(out[..3], [1, 2, 3]);
        unpredict(&[], &mut [], &mut [1], 16, 9);
    }

    proptest! {
        #[test]
        fn undoes_the_predictor(
            bits in 4u32..=33,
            order in prop_oneof![Just(4usize), Just(8), 1usize..31],
            den_shift in 0u32..16,
            seed in prop::collection::vec(any::<i32>(), 1..400),
            coefs in prop::collection::vec(any::<i16>(), 31),
        ) {
            let x: Vec<i32> = seed.iter().map(|&v| sign_extend(v, bits)).collect();
            let mut enc = coefs[..order].to_vec();
            let residual = predict(&x, &mut enc, bits, den_shift);
            let mut dec = coefs[..order].to_vec();
            let mut out = vec![0; x.len()];
            unpredict(&residual, &mut out, &mut dec, bits, den_shift);
            prop_assert_eq!(out, x);
            prop_assert_eq!(dec, enc);
        }
    }
}
