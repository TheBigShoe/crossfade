//! The float build's `mathops.h` and `bands.c`'s integer helpers: every transcendental through
//! `libm`, in double as the reference's macros compute them, so the output is the same bits
//! everywhere.

/// `PI` of `mathops.h`: a float constant, not `std::f32::consts::PI`.
const PI: f32 = 3.141_592_653;

/// `celt_exp2`: `exp(ln 2 * x)` in double.
pub(super) fn exp2(x: f32) -> f32 {
    libm::exp(0.693_147_180_559_945_309_4 * f64::from(x)) as f32
}

/// `celt_cos_norm`: `cos(PI / 2 * x)`, the product in float, the cosine in double.
pub(super) fn cos_norm(x: f32) -> f32 {
    libm::cos(f64::from((0.5 * PI) * x)) as f32
}

/// `celt_lcg_rand`.
pub(super) fn lcg_rand(seed: u32) -> u32 {
    seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223)
}

/// `isqrt32`: `floor(sqrt(val))`, exactly.
pub(super) fn isqrt32(mut val: u32) -> u32 {
    if val == 0 {
        return 0;
    }
    let mut g = 0u32;
    let mut bshift = (32 - val.leading_zeros() as i32 - 1) >> 1;
    let mut b = 1u32 << bshift;
    while bshift >= 0 {
        let t = ((g << 1) + b) << bshift;
        if t <= val {
            g += b;
            val -= t;
        }
        b >>= 1;
        bshift -= 1;
    }
    g
}

/// `FRAC_MUL16`: a Q15 product of the 16-bit truncations of `a` and `b`, as the C casts do.
pub(super) fn frac_mul16(a: i32, b: i32) -> i32 {
    (16384 + i32::from(a as i16) * i32::from(b as i16)) >> 15
}

/// `bitexact_cos`: a cosine approximation in Q15 that decides bit allocation, so exact.
pub(super) fn bitexact_cos(x: i32) -> i32 {
    let tmp = (4096 + x * x) >> 13;
    let x2 = i32::from(tmp as i16);
    let x2 = (32767 - x2) + frac_mul16(x2, -7651 + frac_mul16(x2, 8277 + frac_mul16(-626, x2)));
    i32::from((1 + i32::from(x2 as i16)) as i16)
}

/// `bitexact_log2tan`.
pub(super) fn bitexact_log2tan(isin: i32, icos: i32) -> i32 {
    let lc = 32 - (icos as u32).leading_zeros() as i32;
    let ls = 32 - (isin as u32).leading_zeros() as i32;
    let icos = icos << (15 - lc);
    let isin = isin << (15 - ls);
    (ls - lc) * (1 << 11) + frac_mul16(isin, frac_mul16(isin, -2597) + 7932)
        - frac_mul16(icos, frac_mul16(icos, -2597) + 7932)
}

#[cfg(test)]
#[path = "maths_tests.rs"]
mod tests;
