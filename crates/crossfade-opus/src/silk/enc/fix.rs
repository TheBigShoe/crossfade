//! The fixed-point helpers the encoder needs beyond the decoder's `silk/macros.rs` (libopus
//! `macros.h`, `SigProc_FIX.h`, `Inlines.h`, `lin2log.c`, `sigm_Q15.c`, `sort.c`,
//! `inner_prod_aligned.c`, `sum_sqr_shift.c`).
//!
//! As in the decoder, a C operation that can overflow wraps by name; the `_SAT` ones saturate.

pub(super) use crate::silk::macros::{
    clz_frac, clz32, div32, div32_varq, inverse32_varq, limit, log2lin, lshift, lshift_sat32,
    rshift_round, sat16, silk_rand, smlabb, smlawb, smlaww, smmul, smulbb, smulwb, smulww,
    sqrt_approx,
};

/// `SILK_FIX_CONST` for a C `float` constant (`0.94f`): the product is a float, then rounded
/// up by a half and truncated, as the C's preprocessor arithmetic does.
pub(super) const fn fix_f32(c: f32, q: u32) -> i32 {
    ((c * (1u64 << q) as f32) as f64 + 0.5) as i32
}

/// `SILK_FIX_CONST` for a C `double` constant (`0.94`).
pub(super) const fn fix_f64(c: f64, q: u32) -> i32 {
    (c * (1u64 << q) as f64 + 0.5) as i32
}

/// `silk_SMULWT`: `(a * (b >> 16)) >> 16`.
#[inline(always)]
pub(super) fn smulwt(a: i32, b: i32) -> i32 {
    ((i64::from(a) * i64::from(b >> 16)) >> 16) as i32
}

/// `silk_SMLAWT`, wrapping.
#[inline(always)]
pub(super) fn smlawt(a: i32, b: i32, c: i32) -> i32 {
    a.wrapping_add(smulwt(b, c))
}

/// `silk_SMULL`.
#[inline(always)]
pub(super) fn smull(a: i32, b: i32) -> i64 {
    i64::from(a) * i64::from(b)
}

/// `silk_MLA`, wrapping.
#[inline(always)]
pub(super) fn mla(a: i32, b: i32, c: i32) -> i32 {
    a.wrapping_add(b.wrapping_mul(c))
}

/// `silk_ADD_SAT32`.
#[inline(always)]
pub(super) fn add_sat32(a: i32, b: i32) -> i32 {
    a.saturating_add(b)
}

/// `silk_SUB_SAT32`.
#[inline(always)]
pub(super) fn sub_sat32(a: i32, b: i32) -> i32 {
    a.saturating_sub(b)
}

/// `silk_ADD_POS_SAT32`: for non-negative operands.
#[inline(always)]
pub(super) fn add_pos_sat32(a: i32, b: i32) -> i32 {
    let s = (a as u32).wrapping_add(b as u32);
    if s & 0x8000_0000 != 0 {
        i32::MAX
    } else {
        s as i32
    }
}

/// `silk_ADD_LSHIFT32`, wrapping.
#[inline(always)]
pub(super) fn add_lshift32(a: i32, b: i32, shift: u32) -> i32 {
    a.wrapping_add(lshift(b, shift))
}

/// `silk_SUB_LSHIFT32`, wrapping.
#[inline(always)]
pub(super) fn sub_lshift32(a: i32, b: i32, shift: u32) -> i32 {
    a.wrapping_sub(lshift(b, shift))
}

/// `silk_LSHIFT64`, through `u64`.
#[inline(always)]
pub(super) fn lshift64(a: i64, shift: u32) -> i64 {
    (a as u64).wrapping_shl(shift) as i64
}

/// `silk_CLZ64`.
#[inline(always)]
pub(super) fn clz64(a: i64) -> i32 {
    (a as u64).leading_zeros() as i32
}

/// `silk_DIV32_16` with the C's results where it divides at all; see `div32`.
#[inline(always)]
pub(super) fn div32_16(a: i32, b: i32) -> i32 {
    div32(a, b)
}

/// `silk_lin2log`: about `128 * log2(x)`.
pub(super) fn lin2log(in_lin: i32) -> i32 {
    let (lz, frac_q7) = clz_frac(in_lin);
    add_lshift32(smlawb(frac_q7, frac_q7 * (128 - frac_q7), 179), 31 - lz, 7)
}

/// `silk_sigm_Q15`: a sigmoid, input in Q5, output in Q15.
pub(super) fn sigm_q15(in_q5: i32) -> i32 {
    const SLOPE_Q10: [i32; 6] = [237, 153, 73, 30, 12, 7];
    const POS_Q15: [i32; 6] = [16384, 23955, 28861, 31213, 32178, 32548];
    const NEG_Q15: [i32; 6] = [16384, 8812, 3906, 1554, 589, 219];
    if in_q5 < 0 {
        let x = in_q5.wrapping_neg();
        if x >= 6 * 32 {
            0
        } else {
            let ind = (x >> 5) as usize;
            NEG_Q15[ind] - smulbb(SLOPE_Q10[ind], x & 0x1F)
        }
    } else if in_q5 >= 6 * 32 {
        32767
    } else {
        let ind = (in_q5 >> 5) as usize;
        POS_Q15[ind] + smulbb(SLOPE_Q10[ind], in_q5 & 0x1F)
    }
}

/// `silk_insertion_sort_increasing`: the `idx.len()` smallest of `a`, sorted into its front,
/// with their indices.
pub(super) fn insertion_sort_increasing(a: &mut [i32], idx: &mut [usize]) {
    let k = idx.len();
    for (i, x) in idx.iter_mut().enumerate() {
        *x = i;
    }
    for i in 1..k {
        let value = a[i];
        let mut j = i;
        while j > 0 && value < a[j - 1] {
            a[j] = a[j - 1];
            idx[j] = idx[j - 1];
            j -= 1;
        }
        a[j] = value;
        idx[j] = i;
    }
    for i in k..a.len() {
        let value = a[i];
        if value < a[k - 1] {
            let mut j = k - 1;
            while j > 0 && value < a[j - 1] {
                a[j] = a[j - 1];
                idx[j] = idx[j - 1];
                j -= 1;
            }
            a[j] = value;
            idx[j] = i;
        }
    }
}

/// `silk_insertion_sort_decreasing_int16`: the `idx.len()` largest of `a`, sorted into its
/// front, with their indices.
pub(super) fn insertion_sort_decreasing_i16(a: &mut [i16], idx: &mut [usize]) {
    let k = idx.len();
    for (i, x) in idx.iter_mut().enumerate() {
        *x = i;
    }
    for i in 1..k {
        let value = a[i];
        let mut j = i;
        while j > 0 && value > a[j - 1] {
            a[j] = a[j - 1];
            idx[j] = idx[j - 1];
            j -= 1;
        }
        a[j] = value;
        idx[j] = i;
    }
    for i in k..a.len() {
        let value = a[i];
        if value > a[k - 1] {
            let mut j = k - 1;
            while j > 0 && value > a[j - 1] {
                a[j] = a[j - 1];
                idx[j] = idx[j - 1];
                j -= 1;
            }
            a[j] = value;
            idx[j] = i;
        }
    }
}

/// `silk_inner_prod_aligned` / `silk_inner_prod16`'s 32-bit sibling: wrapping, as the C's
/// accumulation does.
pub(super) fn inner_prod_aligned(a: &[i16], b: &[i16]) -> i32 {
    a.iter()
        .zip(b)
        .fold(0i32, |acc, (&x, &y)| smlabb(acc, x.into(), y.into()))
}

/// `silk_inner_prod_aligned_scale`: each product shifted right by `scale` first.
pub(super) fn inner_prod_aligned_scale(a: &[i16], b: &[i16], scale: u32) -> i32 {
    a.iter().zip(b).fold(0i32, |acc, (&x, &y)| {
        acc.wrapping_add(smulbb(x.into(), y.into()) >> scale)
    })
}

/// `silk_inner_prod16`: in 64 bits.
pub(super) fn inner_prod16(a: &[i16], b: &[i16]) -> i64 {
    a.iter()
        .zip(b)
        .fold(0i64, |acc, (&x, &y)| acc + i64::from(x) * i64::from(y))
}

pub(super) use crate::silk::macros::sum_sqr_shift;
