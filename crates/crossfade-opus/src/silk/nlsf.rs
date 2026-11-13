//! Normalised line spectral frequencies to LPC coefficients (libopus `silk/NLSF_unpack.c`,
//! `NLSF_decode.c`, `NLSF_stabilize.c`, `NLSF2A.c`, `LPC_fit.c`, `LPC_inv_pred_gain.c`,
//! `bwexpander.c`, `bwexpander_32.c`; RFC 6716 4.2.7.5), with RFC 8251's fixes: the
//! inverse prediction gain treats a 64-bit update outside 32 bits as unstable (section 6) and
//! stabilisation adds the minimum spacing with saturation (section 7).

use super::macros::{
    clz32, div32, inverse32_varq, limit, lshift, rshift_round, rshift_round64, sat16, smlawb,
    smmul, smulbb, smulww,
};
use super::state::MAX_LPC_ORDER;
use super::tables_nlsf::{LSFCOSTAB_FIX_Q12, NlsfCodebook};

const NLSF_QUANT_LEVEL_ADJ_Q10: i32 = 102; // SILK_FIX_CONST(0.1, 10)
const MAX_LPC_STABILIZE_ITERATIONS: usize = 16;

/// `silk_NLSF_unpack`: entropy table offsets and backward predictor weights for a first-stage
/// vector.
pub(super) fn nlsf_unpack(
    ec_ix: &mut [i16; MAX_LPC_ORDER],
    pred_q8: &mut [u8; MAX_LPC_ORDER],
    cb: &NlsfCodebook,
    cb1_index: usize,
) {
    let sel = &cb.ec_sel[cb1_index * cb.order / 2..];
    for i in (0..cb.order).step_by(2) {
        let entry = sel[i / 2];
        ec_ix[i] = i16::from((entry >> 1) & 7) * 9;
        pred_q8[i] = cb.pred_q8[i + usize::from(entry & 1) * (cb.order - 1)];
        ec_ix[i + 1] = i16::from((entry >> 5) & 7) * 9;
        pred_q8[i + 1] = cb.pred_q8[i + usize::from((entry >> 4) & 1) * (cb.order - 1) + 1];
    }
}

/// `silk_NLSF_decode`: the quantised NLSF vector from its indices.
pub(super) fn nlsf_decode(nlsf_q15: &mut [i16; MAX_LPC_ORDER], indices: &[i8], cb: &NlsfCodebook) {
    let cb1 = (indices[0].max(0) as usize).min(cb.n_vectors - 1);
    let mut ec_ix = [0i16; MAX_LPC_ORDER];
    let mut pred_q8 = [0u8; MAX_LPC_ORDER];
    nlsf_unpack(&mut ec_ix, &mut pred_q8, cb, cb1);

    // silk_NLSF_residual_dequant: backwards prediction, in the C's 16-bit casts.
    let mut res_q10 = [0i16; MAX_LPC_ORDER];
    let mut out_q10 = 0i32;
    for i in (0..cb.order).rev() {
        let pred_q10 = smulbb(out_q10, i32::from(pred_q8[i])) >> 8;
        out_q10 = i32::from(indices[i + 1]) << 10;
        if out_q10 > 0 {
            out_q10 -= NLSF_QUANT_LEVEL_ADJ_Q10;
        } else if out_q10 < 0 {
            out_q10 += NLSF_QUANT_LEVEL_ADJ_Q10;
        }
        out_q10 = smlawb(pred_q10, out_q10, cb.quant_step_size_q16);
        res_q10[i] = out_q10 as i16;
    }

    let base = cb1 * cb.order;
    for i in 0..cb.order {
        let w = i32::from(cb.cb1_wght_q9[base + i]);
        let tmp =
            div32(i32::from(res_q10[i]) << 14, w) + (i32::from(cb.cb1_nlsf_q8[base + i]) << 7);
        nlsf_q15[i] = limit(tmp, 0, 32767) as i16;
    }
    nlsf_stabilize(&mut nlsf_q15[..cb.order], cb.delta_min_q15);
}

/// `silk_NLSF_stabilize`: at least `delta_min[i]` between neighbours and from 0 and 1.
pub(super) fn nlsf_stabilize(nlsf: &mut [i16], delta_min: &[i16]) {
    const MAX_LOOPS: usize = 20;
    let l = nlsf.len();
    let dm = |i: usize| i32::from(delta_min[i]);
    for _ in 0..MAX_LOOPS {
        let mut min_diff = i32::from(nlsf[0]) - dm(0);
        let mut at = 0;
        for i in 1..l {
            let diff = i32::from(nlsf[i]) - (i32::from(nlsf[i - 1]) + dm(i));
            if diff < min_diff {
                min_diff = diff;
                at = i;
            }
        }
        let diff = (1 << 15) - (i32::from(nlsf[l - 1]) + dm(l));
        if diff < min_diff {
            min_diff = diff;
            at = l;
        }
        if min_diff >= 0 {
            return;
        }
        if at == 0 {
            nlsf[0] = delta_min[0];
        } else if at == l {
            nlsf[l - 1] = ((1 << 15) - dm(l)) as i16;
        } else {
            let mut min_center = 0;
            for k in 0..at {
                min_center += dm(k);
            }
            min_center += dm(at) >> 1;
            let mut max_center = 1 << 15;
            for k in (at + 1..=l).rev() {
                max_center -= dm(k);
            }
            max_center -= dm(at) >> 1;
            let center = limit(
                rshift_round(i32::from(nlsf[at - 1]) + i32::from(nlsf[at]), 1),
                min_center,
                max_center,
            ) as i16;
            nlsf[at - 1] = (i32::from(center) - (dm(at) >> 1)) as i16;
            nlsf[at] = (i32::from(nlsf[at - 1]) + dm(at)) as i16;
        }
    }
    // Fall back to brute force (silk_insertion_sort_increasing_all_values_int16 and clamps).
    nlsf.sort_unstable();
    nlsf[0] = nlsf[0].max(delta_min[0]);
    for i in 1..l {
        // RFC 8251 section 7: saturate, or the sum wraps.
        nlsf[i] = nlsf[i].max(sat16(i32::from(nlsf[i - 1]) + dm(i)));
    }
    nlsf[l - 1] = (i32::from(nlsf[l - 1]).min((1 << 15) - dm(l))) as i16;
    for i in (0..l - 1).rev() {
        nlsf[i] = (i32::from(nlsf[i]).min(i32::from(nlsf[i + 1]) - dm(i + 1))) as i16;
    }
}

/// `silk_bwexpander`: chirps a 16-bit filter.
pub(super) fn bwexpander(ar: &mut [i16], mut chirp_q16: i32) {
    let chirp_minus_one = chirp_q16 - 65536;
    let d = ar.len();
    for a in &mut ar[..d - 1] {
        *a = rshift_round(chirp_q16.wrapping_mul(i32::from(*a)), 16) as i16;
        chirp_q16 += rshift_round(chirp_q16 * chirp_minus_one, 16);
    }
    ar[d - 1] = rshift_round(chirp_q16.wrapping_mul(i32::from(ar[d - 1])), 16) as i16;
}

/// `silk_bwexpander_32`: chirps a 32-bit filter.
pub(super) fn bwexpander_32(ar: &mut [i32], mut chirp_q16: i32) {
    let chirp_minus_one = chirp_q16 - 65536;
    let d = ar.len();
    for a in &mut ar[..d - 1] {
        *a = smulww(chirp_q16, *a);
        chirp_q16 += rshift_round(chirp_q16 * chirp_minus_one, 16);
    }
    ar[d - 1] = smulww(chirp_q16, ar[d - 1]);
}

/// `silk_LPC_fit`: 32-bit coefficients in `q_in` to 16-bit ones in `q_out`, chirping until
/// they fit (saturating after ten tries).
pub(super) fn lpc_fit(a_out: &mut [i16], a_in: &mut [i32], q_out: u32, q_in: u32) {
    let d = a_in.len();
    let mut fitted = false;
    for _ in 0..10 {
        let mut maxabs = 0;
        let mut idx = 0;
        for (k, &a) in a_in.iter().enumerate() {
            let absval = a.wrapping_abs();
            if absval > maxabs {
                maxabs = absval;
                idx = k;
            }
        }
        let maxabs = rshift_round(maxabs, q_in - q_out);
        if maxabs > i32::from(i16::MAX) {
            let maxabs = maxabs.min(163_838);
            let chirp_q16 = 65470 // SILK_FIX_CONST(0.999, 16)
                - div32(
                    (maxabs - i32::from(i16::MAX)) << 14,
                    (maxabs * (idx as i32 + 1)) >> 2,
                );
            bwexpander_32(a_in, chirp_q16);
        } else {
            fitted = true;
            break;
        }
    }
    if fitted {
        for k in 0..d {
            a_out[k] = rshift_round(a_in[k], q_in - q_out) as i16;
        }
    } else {
        for k in 0..d {
            a_out[k] = sat16(rshift_round(a_in[k], q_in - q_out));
            a_in[k] = lshift(i32::from(a_out[k]), q_in - q_out);
        }
    }
}

/// `silk_NLSF2A`: NLSFs in Q15 to a stable whitening filter in Q12.
pub(super) fn nlsf2a(a_q12: &mut [i16], nlsf: &[i16]) {
    const ORDERING16: [usize; 16] = [0, 15, 8, 7, 4, 11, 12, 3, 2, 13, 10, 5, 6, 9, 14, 1];
    const ORDERING10: [usize; 10] = [0, 9, 6, 3, 4, 5, 8, 1, 2, 7];
    const QA: u32 = 16;
    let d = nlsf.len();
    let ordering: &[usize] = if d == 16 { &ORDERING16 } else { &ORDERING10 };
    let mut cos_lsf_qa = [0i32; MAX_LPC_ORDER];
    for k in 0..d {
        // Every NLSF reaching here lies in 0..=32767; the clamp only guards the table.
        let f = i32::from(nlsf[k].max(0));
        let f_int = (f >> 8) as usize;
        let f_frac = f - ((f_int as i32) << 8);
        let cos_val = i32::from(LSFCOSTAB_FIX_Q12[f_int]);
        let delta = i32::from(LSFCOSTAB_FIX_Q12[f_int + 1]) - cos_val;
        cos_lsf_qa[ordering[k]] = rshift_round((cos_val << 8) + delta * f_frac, 20 - QA);
    }
    let dd = d / 2;
    let mut p = [0i32; MAX_LPC_ORDER / 2 + 1];
    let mut q = [0i32; MAX_LPC_ORDER / 2 + 1];
    find_poly(&mut p, &cos_lsf_qa[0..], dd);
    find_poly(&mut q, &cos_lsf_qa[1..], dd);
    let mut a32_qa1 = [0i32; MAX_LPC_ORDER];
    for k in 0..dd {
        let ptmp = p[k + 1].wrapping_add(p[k]);
        let qtmp = q[k + 1].wrapping_sub(q[k]);
        a32_qa1[k] = qtmp.wrapping_neg().wrapping_sub(ptmp);
        a32_qa1[d - k - 1] = qtmp.wrapping_sub(ptmp);
    }
    let a32 = &mut a32_qa1[..d];
    lpc_fit(a_q12, a32, 12, QA + 1);
    let mut i = 0;
    while lpc_inverse_pred_gain(&a_q12[..d]) == 0 && i < MAX_LPC_STABILIZE_ITERATIONS {
        bwexpander_32(a32, 65536 - (2 << i));
        for k in 0..d {
            a_q12[k] = rshift_round(a32[k], QA + 1 - 12) as i16;
        }
        i += 1;
    }
}

/// `silk_NLSF2A_find_poly`.
fn find_poly(out: &mut [i32], c_lsf: &[i32], dd: usize) {
    const QA: u32 = 16;
    let mul = |a: i32, b: i32| rshift_round64(i64::from(a) * i64::from(b), QA) as i32;
    out[0] = 1 << QA;
    out[1] = -c_lsf[0];
    for k in 1..dd {
        let ftmp = c_lsf[2 * k];
        out[k + 1] = lshift(out[k - 1], 1).wrapping_sub(mul(ftmp, out[k]));
        for n in (2..=k).rev() {
            out[n] = out[n].wrapping_add(out[n - 2].wrapping_sub(mul(ftmp, out[n - 1])));
        }
        out[1] = out[1].wrapping_sub(ftmp);
    }
}

/// `silk_LPC_inverse_pred_gain`: the inverse prediction gain in Q30, or 0 for a filter that
/// is unstable or whose gain exceeds `MAX_PREDICTION_POWER_GAIN`.
pub(super) fn lpc_inverse_pred_gain(a_q12: &[i16]) -> i32 {
    const QA: u32 = 24;
    let mut a_qa = [0i32; MAX_LPC_ORDER];
    let mut dc_resp = 0;
    for (t, &a) in a_qa.iter_mut().zip(a_q12) {
        dc_resp += i32::from(a);
        *t = i32::from(a) << (QA - 12);
    }
    if dc_resp >= 4096 {
        return 0;
    }
    inverse_pred_gain_qa(&mut a_qa[..a_q12.len()])
}

/// `LPC_inverse_pred_gain_QA_c`, with RFC 8251 section 6's overflow check.
fn inverse_pred_gain_qa(a: &mut [i32]) -> i32 {
    const QA: u32 = 24;
    const A_LIMIT: i32 = 16_773_022; // SILK_FIX_CONST(0.99975, QA)
    const MIN_INV_GAIN_Q30: i32 = 107_374; // SILK_FIX_CONST(1.0f / MAX_PREDICTION_POWER_GAIN, 30)
    let mul_frac = |a: i32, b: i32, q: u32| rshift_round64(i64::from(a) * i64::from(b), q) as i32;
    let order = a.len();
    let mut inv_gain_q30: i32 = 1 << 30;
    for k in (1..order).rev() {
        if a[k] > A_LIMIT || a[k] < -A_LIMIT {
            return 0;
        }
        let rc_q31 = -(a[k] << (31 - QA));
        let rc_mult1_q30 = (1 << 30) - smmul(rc_q31, rc_q31);
        inv_gain_q30 = smmul(inv_gain_q30, rc_mult1_q30) << 2;
        if inv_gain_q30 < MIN_INV_GAIN_Q30 {
            return 0;
        }
        let mult2q = 32 - clz32(rc_mult1_q30.wrapping_abs());
        let rc_mult2 = inverse32_varq(rc_mult1_q30, mult2q + 30);
        for n in 0..k.div_ceil(2) {
            let (t1, t2) = (a[n], a[k - n - 1]);
            let up = |x: i32, y: i32| {
                rshift_round64(
                    i64::from(x.saturating_sub(mul_frac(y, rc_q31, 31))) * i64::from(rc_mult2),
                    mult2q as u32,
                )
            };
            let v1 = up(t1, t2);
            if v1 > i64::from(i32::MAX) || v1 < i64::from(i32::MIN) {
                return 0;
            }
            a[n] = v1 as i32;
            let v2 = up(t2, t1);
            if v2 > i64::from(i32::MAX) || v2 < i64::from(i32::MIN) {
                return 0;
            }
            a[k - n - 1] = v2 as i32;
        }
    }
    if a[0] > A_LIMIT || a[0] < -A_LIMIT {
        return 0;
    }
    let rc_q31 = -(a[0] << (31 - QA));
    let rc_mult1_q30 = (1 << 30) - smmul(rc_q31, rc_q31);
    inv_gain_q30 = smmul(inv_gain_q30, rc_mult1_q30) << 2;
    if inv_gain_q30 < MIN_INV_GAIN_Q30 {
        return 0;
    }
    inv_gain_q30
}

#[cfg(test)]
#[path = "nlsf_tests.rs"]
mod tests;
