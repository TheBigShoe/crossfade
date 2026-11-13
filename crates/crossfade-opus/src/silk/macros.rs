//! libopus `silk/macros.h`, `SigProc_FIX.h` and `Inlines.h` as small functions (the plain C
//! versions, no platform assembly).
//!
//! C's signed overflow is undefined; libopus relies on two's-complement wrapping wherever a
//! hostile stream can overflow (the `_ovflw` macros say so explicitly, the accumulating ones
//! like `silk_SMLAWB` do so silently). Here every such operation wraps, by name; the `_SAT`
//! macros saturate. The multiply-and-shift macros are computed exactly in 64 bits, which is
//! what the C's split 16-bit products add up to.

/// `(a * (i16)b) >> 16`.
#[inline(always)]
pub(super) fn smulwb(a: i32, b: i32) -> i32 {
    ((i64::from(a) * i64::from(b as i16)) >> 16) as i32
}

/// `a + ((b * (i16)c) >> 16)`, wrapping.
#[inline(always)]
pub(super) fn smlawb(a: i32, b: i32, c: i32) -> i32 {
    a.wrapping_add(smulwb(b, c))
}

/// `(a * b) >> 16` in full (`silk_SMULWW`), wrapping to 32 bits as the C's `silk_MLA` does.
#[inline(always)]
pub(super) fn smulww(a: i32, b: i32) -> i32 {
    ((i64::from(a) * i64::from(b)) >> 16) as i32
}

/// `a + ((b * c) >> 16)`, wrapping.
#[inline(always)]
pub(super) fn smlaww(a: i32, b: i32, c: i32) -> i32 {
    a.wrapping_add(smulww(b, c))
}

/// `(i16)a * (i16)b`: never overflows.
#[inline(always)]
pub(super) fn smulbb(a: i32, b: i32) -> i32 {
    i32::from(a as i16) * i32::from(b as i16)
}

/// `a + (i16)b * (i16)c`, wrapping (`silk_SMLABB` and `silk_SMLABB_ovflw`).
#[inline(always)]
pub(super) fn smlabb(a: i32, b: i32, c: i32) -> i32 {
    a.wrapping_add(smulbb(b, c))
}

/// `(a >> 16) * (b >> 16)`: never overflows.
#[inline(always)]
pub(super) fn smultt(a: i32, b: i32) -> i32 {
    (a >> 16) * (b >> 16)
}

/// `(a * b) >> 32`.
#[inline(always)]
pub(super) fn smmul(a: i32, b: i32) -> i32 {
    ((i64::from(a) * i64::from(b)) >> 32) as i32
}

/// `silk_LSHIFT`: through `u32`, so bits shifted out are lost as in the C.
#[inline(always)]
pub(super) fn lshift(a: i32, shift: u32) -> i32 {
    (a as u32).wrapping_shl(shift) as i32
}

/// `silk_RSHIFT_ROUND`: shift right, rounding half up (`shift` 1 to 31).
#[inline(always)]
pub(super) fn rshift_round(a: i32, shift: u32) -> i32 {
    if shift == 1 {
        (a >> 1) + (a & 1)
    } else {
        ((a >> (shift - 1)) + 1) >> 1
    }
}

/// `silk_RSHIFT_ROUND64` (`shift` 1 to 63).
#[inline(always)]
pub(super) fn rshift_round64(a: i64, shift: u32) -> i64 {
    if shift == 1 {
        (a >> 1) + (a & 1)
    } else {
        ((a >> (shift - 1)) + 1) >> 1
    }
}

/// `silk_SAT16`.
#[inline(always)]
pub(super) fn sat16(a: i32) -> i16 {
    a.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// `silk_ADD_SAT16`.
#[inline(always)]
pub(super) fn add_sat16(a: i16, b: i32) -> i16 {
    sat16(i32::from(a).saturating_add(b))
}

/// `silk_LIMIT`: `a` clamped between two limits given in either order.
#[inline(always)]
pub(super) fn limit(a: i32, limit1: i32, limit2: i32) -> i32 {
    if limit1 > limit2 {
        if a > limit1 {
            limit1
        } else if a < limit2 {
            limit2
        } else {
            a
        }
    } else if a > limit2 {
        limit2
    } else if a < limit1 {
        limit1
    } else {
        a
    }
}

/// `silk_LSHIFT_SAT32`: shift left, saturating (`shift` below 32).
#[inline(always)]
pub(super) fn lshift_sat32(a: i32, shift: u32) -> i32 {
    let shift = shift.min(31);
    lshift(limit(a, i32::MIN >> shift, i32::MAX >> shift), shift)
}

/// `silk_CLZ32`: leading zeros, 32 for 0 (and 0 for a negative value).
#[inline(always)]
pub(super) fn clz32(a: i32) -> i32 {
    (a as u32).leading_zeros() as i32
}

/// `silk_CLZ_FRAC`: leading zeros and the 7 bits after the leading one.
#[inline(always)]
pub(super) fn clz_frac(a: i32) -> (i32, i32) {
    let lz = clz32(a);
    // silk_ROR32(a, 24 - lz): a negative rotation is a left rotation.
    let frac = (a as u32).rotate_right((24 - lz).rem_euclid(32) as u32) as i32 & 0x7f;
    (lz, frac)
}

/// `silk_SQRT_APPROX`: about `sqrt(a)`, 0 for `a <= 0`.
pub(super) fn sqrt_approx(a: i32) -> i32 {
    if a <= 0 {
        return 0;
    }
    let (lz, frac) = clz_frac(a);
    let mut y = if lz & 1 != 0 { 32768 } else { 46214 };
    y >>= lz >> 1;
    smlawb(y, y, smulbb(213, frac))
}

/// `silk_DIV32_16` / `silk_DIV32`: C division (truncating), with the divisions the C cannot
/// survive (by zero, `i32::MIN / -1`) given defined results instead; no valid stream reaches
/// them.
#[inline(always)]
pub(super) fn div32(a: i32, b: i32) -> i32 {
    if b == 0 {
        return if a >= 0 { i32::MAX } else { i32::MIN };
    }
    a.wrapping_div(b)
}

/// `silk_DIV32_varQ`: about `(a << q) / b`.
pub(super) fn div32_varq(a: i32, b: i32, q: i32) -> i32 {
    // Only a zero denominator could break the C; no caller passes one.
    let b = if b == 0 { 1 } else { b };
    let a_headrm = (clz32(a.wrapping_abs()) - 1).max(0);
    let a_nrm = lshift(a, a_headrm as u32);
    let b_headrm = (clz32(b.wrapping_abs()) - 1).max(0);
    let b_nrm = lshift(b, b_headrm as u32);
    let b_inv = div32(i32::MAX >> 2, b_nrm >> 16);
    let mut result = smulwb(a_nrm, b_inv);
    // The residual may wrap: its final value is small (libopus says so).
    let a_nrm = a_nrm.wrapping_sub(lshift(smmul(b_nrm, result), 3));
    result = smlawb(result, a_nrm, b_inv);
    let shift = 29 + a_headrm - b_headrm - q;
    if shift < 0 {
        lshift_sat32(result, (-shift) as u32)
    } else if shift < 32 {
        result >> shift
    } else {
        0
    }
}

/// `silk_INVERSE32_varQ`: about `(1 << q) / b`.
pub(super) fn inverse32_varq(b: i32, q: i32) -> i32 {
    let b = if b == 0 { 1 } else { b };
    let b_headrm = (clz32(b.wrapping_abs()) - 1).max(0);
    let b_nrm = lshift(b, b_headrm as u32);
    let b_inv = div32(i32::MAX >> 2, b_nrm >> 16);
    let mut result = lshift(b_inv, 16);
    let err_q32 = lshift((1i32 << 29).wrapping_sub(smulwb(b_nrm, b_inv)), 3);
    result = smlaww(result, err_q32, b_inv);
    let shift = 61 - b_headrm - q;
    if shift <= 0 {
        lshift_sat32(result, (-shift) as u32)
    } else if shift < 32 {
        result >> shift
    } else {
        0
    }
}

/// `silk_RAND`: the linear congruential generator, wrapping.
#[inline(always)]
pub(super) fn silk_rand(seed: i32) -> i32 {
    907_633_515_i32.wrapping_add(seed.wrapping_mul(196_314_165))
}

/// `silk_log2lin`: about `2 ** (x / 128)`.
pub(super) fn log2lin(in_log_q7: i32) -> i32 {
    if in_log_q7 < 0 {
        return 0;
    }
    if in_log_q7 >= 31 << 7 {
        // Every caller limits its input to 3967; the C's shift would overflow past it.
        return i32::MAX;
    }
    let mut out = 1 << (in_log_q7 >> 7);
    let frac = in_log_q7 & 0x7f;
    let poly = smlawb(frac, smulbb(frac, 128 - frac), -174);
    if in_log_q7 < 2048 {
        out += (out * poly) >> 7;
    } else {
        out += (out >> 7) * poly;
    }
    out
}

/// `silk_sum_sqr_shift`: the energy of `x` and the right shift that keeps it in 31 bits with
/// two bits of headroom.
pub(super) fn sum_sqr_shift(x: &[i16]) -> (i32, i32) {
    let len = x.len() as i32;
    let pass = |shft: i32| {
        let mut nrg: u32 = 0;
        let (pairs, rest) = x.as_chunks::<2>();
        for p in pairs {
            let t = smlabb(smulbb(p[0].into(), p[0].into()), p[1].into(), p[1].into()) as u32;
            nrg = nrg.wrapping_add(t >> shft);
        }
        if let [last] = rest {
            let t = smulbb((*last).into(), (*last).into()) as u32;
            nrg = nrg.wrapping_add(t >> shft);
        }
        nrg
    };
    let shft = 31 - clz32(len);
    let nrg = pass(shft).wrapping_add(len as u32);
    let shft = (shft + 3 - clz32(nrg as i32)).max(0);
    (pass(shft) as i32, shft)
}

#[cfg(test)]
#[path = "macros_tests.rs"]
mod tests;
