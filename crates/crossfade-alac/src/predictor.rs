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
    let num = residual.len().min(out.len());
    if num == 0 {
        return;
    }
    let (residual, out) = (&residual[..num], &mut out[..num]);
    out[0] = residual[0];
    let order = coefs.len();
    if order == 0 {
        out.copy_from_slice(residual);
        return;
    }
    if order == FIRST_DIFFERENCE {
        out.copy_from_slice(residual);
        integrate(out, chan_bits);
        return;
    }
    for j in 1..=order.min(num - 1) {
        out[j] = sign_extend(residual[j].wrapping_add(out[j - 1]), chan_bits);
    }
    // The orders encoders use get a copy with the taps in registers; the arithmetic is the same.
    match order {
        4 => fixed::<4>(residual, out, coefs, chan_bits, den_shift),
        8 => fixed::<8>(residual, out, coefs, chan_bits, den_shift),
        16 => fixed::<16>(residual, out, coefs, chan_bits, den_shift),
        _ => filter(residual, out, coefs, order, chan_bits, den_shift),
    }
}

/// `filter` for an order known at compile time, with the coefficients and taps in arrays.
fn fixed<const N: usize>(
    residual: &[i32],
    out: &mut [i32],
    coefs: &mut [i16],
    chan_bits: u32,
    den_shift: u32,
) {
    let Ok(c) = <&[i16; N]>::try_from(&*coefs) else {
        return;
    };
    let mut c = *c;
    let den_half = if den_shift == 0 {
        0
    } else {
        1i32 << (den_shift - 1)
    };
    // Sign extension from `chan_bits` as a pair of shifts (none at 32 bits and more).
    let wrap = 32 - chan_bits.min(32);
    for j in N + 1..out.len() {
        let top = out[j - N - 1];
        let Ok(taps) = <[i32; N]>::try_from(&out[j - N..j]) else {
            return;
        };
        let mut sum = 0i32;
        for k in 0..N {
            sum = sum.wrapping_add(i32::from(c[k]).wrapping_mul(taps[N - 1 - k].wrapping_sub(top)));
        }
        let del = residual[j];
        let predicted = sum.wrapping_add(den_half) >> den_shift;
        out[j] = (del.wrapping_add(top).wrapping_add(predicted) << wrap) >> wrap;
        let mut left = del;
        if del > 0 {
            for k in (0..N).rev() {
                let dd = top.wrapping_sub(taps[N - 1 - k]);
                let sgn = dd.signum();
                c[k] = c[k].wrapping_sub(sgn as i16);
                left = left
                    .wrapping_sub(((N - k) as i32).wrapping_mul(sgn.wrapping_mul(dd) >> den_shift));
                if left <= 0 {
                    break;
                }
            }
        } else if del < 0 {
            for k in (0..N).rev() {
                let dd = top.wrapping_sub(taps[N - 1 - k]);
                let sgn = dd.signum();
                c[k] = c[k].wrapping_add(sgn as i16);
                left = left.wrapping_sub(
                    ((N - k) as i32).wrapping_mul(sgn.wrapping_neg().wrapping_mul(dd) >> den_shift),
                );
                if left >= 0 {
                    break;
                }
            }
        }
    }
    coefs.copy_from_slice(&c);
}

/// In place: each sample becomes itself plus the one before it, wrapped to `chan_bits`.
pub(crate) fn integrate(buf: &mut [i32], chan_bits: u32) {
    let Some((&mut first, rest)) = buf.split_first_mut() else {
        return;
    };
    let mut prev = first;
    for v in rest {
        prev = sign_extend(v.wrapping_add(prev), chan_bits);
        *v = prev;
    }
}

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
            order in prop_oneof![Just(4usize), Just(8), Just(16), 1usize..31],
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
