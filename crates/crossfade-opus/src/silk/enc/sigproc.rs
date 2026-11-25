//! The encoder's signal processing (libopus `silk/fixed/apply_sine_window_FIX.c`,
//! `autocorr_FIX.c` with celt's `_celt_autocorr`, `schur_FIX.c`, `schur64_FIX.c`, `k2a_FIX.c`,
//! `k2a_Q16_FIX.c`, `warped_autocorrelation_FIX.c`, `burg_modified_FIX.c`, `corrMatrix_FIX.c`,
//! `find_LTP_FIX.c`, `residual_energy_FIX.c`, `LTP_analysis_filter_FIX.c`, `vector_ops_FIX.c`).

use super::fix::{
    add_lshift32, clz32, clz64, div32, div32_varq, fix_f32, inner_prod_aligned, inner_prod16,
    lshift, lshift64, mla, rshift_round, sat16, smlabb, smlawb, smlaww, smmul, smulbb, smull,
    smulwb, sqrt_approx, sum_sqr_shift,
};
use crate::silk::decode_core::lpc_analysis_filter;
use crate::silk::state::{LTP_ORDER, MAX_LPC_ORDER, MAX_NB_SUBFR};

pub(super) const MAX_SHAPE_LPC_ORDER: usize = 24;
const QC: i32 = 10;
const QS: i32 = 13;
const FIND_LPC_COND_FAC: f32 = 1e-5;
const LTP_CORR_INV_MAX: f32 = 0.03;

/// `silk_apply_sine_window`: the rising (`win_type` 1) or falling (2) half of a sine window
/// over `length` samples (a multiple of 4 from 16 to 120).
pub(super) fn apply_sine_window(px_win: &mut [i16], px: &[i16], win_type: i32, length: usize) {
    const FREQ_TABLE_Q16: [i32; 27] = [
        12111, 9804, 8235, 7100, 6239, 5565, 5022, 4575, 4202, 3885, 3612, 3375, 3167, 2984, 2820,
        2674, 2542, 2422, 2313, 2214, 2123, 2038, 1961, 1889, 1822, 1760, 1702,
    ];
    let f_q16 = FREQ_TABLE_Q16[(length >> 2) - 4];
    let c_q16 = smulwb(f_q16, -f_q16);
    let (mut s0_q16, mut s1_q16) = if win_type == 1 {
        (0, f_q16 + (length as i32 >> 3))
    } else {
        (1 << 16, (1 << 16) + (c_q16 >> 1) + (length as i32 >> 4))
    };
    let mut k = 0;
    while k < length {
        px_win[k] = smulwb((s0_q16 + s1_q16) >> 1, px[k].into()) as i16;
        px_win[k + 1] = smulwb(s1_q16, px[k + 1].into()) as i16;
        s0_q16 = smulwb(s1_q16, c_q16) + (s1_q16 << 1) - s0_q16 + 1;
        s0_q16 = s0_q16.min(1 << 16);
        px_win[k + 2] = smulwb((s0_q16 + s1_q16) >> 1, px[k + 2].into()) as i16;
        px_win[k + 3] = smulwb(s0_q16, px[k + 3].into()) as i16;
        s1_q16 = smulwb(s0_q16, c_q16) + (s0_q16 << 1) - s1_q16;
        s1_q16 = s1_q16.min(1 << 16);
        k += 4;
    }
}

/// `silk_autocorr` (celt's fixed-point `_celt_autocorr`, no window): `results.len()`
/// correlations of `x`, normalised; returns their scale.
pub(super) fn autocorr(results: &mut [i32], x: &[i16]) -> i32 {
    let n = x.len();
    let lag = results.len().min(n) - 1;
    let mut ac0: i32 = 1 + ((n as i32) << 7);
    if n & 1 != 0 {
        ac0 = ac0.wrapping_add(smulbb(x[0].into(), x[0].into()) >> 9);
    }
    let mut i = n & 1;
    while i < n {
        ac0 = ac0.wrapping_add(smulbb(x[i].into(), x[i].into()) >> 9);
        ac0 = ac0.wrapping_add(smulbb(x[i + 1].into(), x[i + 1].into()) >> 9);
        i += 2;
    }
    // celt_ilog2, then C's truncating division.
    let mut shift = ((31 - clz32(ac0)) - 30 + 10) / 2;
    let mut scaled = [0i16; 512];
    let xs: &[i16] = if shift > 0 {
        for (s, &v) in scaled.iter_mut().zip(x) {
            *s = ((i32::from(v) + (1 << shift >> 1)) >> shift) as i16;
        }
        &scaled[..n]
    } else {
        shift = 0;
        x
    };
    for k in 0..=lag {
        results[k] = inner_prod_aligned(&xs[k..], &xs[..n - k]);
    }
    shift *= 2;
    if shift <= 0 {
        results[0] = results[0].wrapping_add(1 << -shift);
    }
    if results[0] < 268_435_456 {
        let shift2 = 29 - (32 - clz32(results[0]));
        for r in results[..=lag].iter_mut() {
            *r = lshift(*r, shift2 as u32);
        }
        shift -= shift2;
    } else if results[0] >= 536_870_912 {
        let shift2 = if results[0] >= 1_073_741_824 { 2 } else { 1 };
        for r in results[..=lag].iter_mut() {
            *r >>= shift2;
        }
        shift += shift2;
    }
    for r in results[lag + 1..].iter_mut() {
        *r = 0;
    }
    shift
}

/// `silk_schur`: reflection coefficients in Q15 from correlations; returns the residual
/// energy.
pub(super) fn schur(rc_q15: &mut [i16], c: &[i32], order: usize) -> i32 {
    let mut cc = [[0i32; 2]; MAX_LPC_ORDER + 1];
    let lz = clz32(c[0]);
    for k in 0..=order {
        let v = if lz < 2 {
            c[k] >> 1
        } else if lz > 2 {
            lshift(c[k], (lz - 2) as u32)
        } else {
            c[k]
        };
        cc[k] = [v, v];
    }
    let mut k = 0;
    while k < order {
        if cc[k + 1][0].wrapping_abs() >= cc[0][1] {
            rc_q15[k] = if cc[k + 1][0] > 0 {
                -fix_f32(0.99, 15) as i16
            } else {
                fix_f32(0.99, 15) as i16
            };
            k += 1;
            break;
        }
        let rc_tmp_q15 = i32::from(sat16(-div32(cc[k + 1][0], (cc[0][1] >> 15).max(1))));
        rc_q15[k] = rc_tmp_q15 as i16;
        for n in 0..order - k {
            let ctmp1 = cc[n + k + 1][0];
            let ctmp2 = cc[n][1];
            cc[n + k + 1][0] = smlawb(ctmp1, lshift(ctmp2, 1), rc_tmp_q15);
            cc[n][1] = smlawb(ctmp2, lshift(ctmp1, 1), rc_tmp_q15);
        }
        k += 1;
    }
    for r in rc_q15[k..order].iter_mut() {
        *r = 0;
    }
    cc[0][1].max(1)
}

/// `silk_schur64`: reflection coefficients in Q16, more accurately.
pub(super) fn schur64(rc_q16: &mut [i32], c: &[i32], order: usize) -> i32 {
    if c[0] <= 0 {
        rc_q16[..order].fill(0);
        return 0;
    }
    let mut cc = [[0i32; 2]; MAX_SHAPE_LPC_ORDER + 1];
    for k in 0..=order {
        cc[k] = [c[k], c[k]];
    }
    let mut k = 0;
    while k < order {
        if cc[k + 1][0].wrapping_abs() >= cc[0][1] {
            rc_q16[k] = if cc[k + 1][0] > 0 {
                -fix_f32(0.99, 16)
            } else {
                fix_f32(0.99, 16)
            };
            k += 1;
            break;
        }
        let rc_tmp_q31 = div32_varq(cc[k + 1][0].wrapping_neg(), cc[0][1], 31);
        rc_q16[k] = rshift_round(rc_tmp_q31, 15);
        for n in 0..order - k {
            let c1 = cc[n + k + 1][0];
            let c2 = cc[n][1];
            cc[n + k + 1][0] = c1.wrapping_add(smmul(lshift(c2, 1), rc_tmp_q31));
            cc[n][1] = c2.wrapping_add(smmul(lshift(c1, 1), rc_tmp_q31));
        }
        k += 1;
    }
    for r in rc_q16[k..order].iter_mut() {
        *r = 0;
    }
    cc[0][1].max(1)
}

/// `silk_k2a`: Q15 reflection coefficients to Q24 prediction coefficients.
pub(super) fn k2a(a_q24: &mut [i32], rc_q15: &[i16]) {
    for k in 0..rc_q15.len() {
        let rc = i32::from(rc_q15[k]);
        for n in 0..(k + 1) >> 1 {
            let tmp1 = a_q24[n];
            let tmp2 = a_q24[k - n - 1];
            a_q24[n] = smlawb(tmp1, lshift(tmp2, 1), rc);
            a_q24[k - n - 1] = smlawb(tmp2, lshift(tmp1, 1), rc);
        }
        a_q24[k] = -lshift(rc, 9);
    }
}

/// `silk_k2a_Q16`: Q16 reflection coefficients to Q24 prediction coefficients.
pub(super) fn k2a_q16(a_q24: &mut [i32], rc_q16: &[i32]) {
    for k in 0..rc_q16.len() {
        let rc = rc_q16[k];
        for n in 0..(k + 1) >> 1 {
            let tmp1 = a_q24[n];
            let tmp2 = a_q24[k - n - 1];
            a_q24[n] = smlaww(tmp1, tmp2, rc);
            a_q24[k - n - 1] = smlaww(tmp2, tmp1, rc);
        }
        a_q24[k] = lshift(rc, 8).wrapping_neg();
    }
}

/// `silk_warped_autocorrelation_FIX_c`: correlations on a frequency axis warped by
/// `warping_q16`; returns their scale.
pub(super) fn warped_autocorrelation(
    corr: &mut [i32],
    input: &[i16],
    warping_q16: i32,
    order: usize,
) -> i32 {
    let mut state_qs = [0i32; MAX_SHAPE_LPC_ORDER + 1];
    let mut corr_qc = [0i64; MAX_SHAPE_LPC_ORDER + 1];
    for &x in input {
        let mut tmp1_qs = lshift(i32::from(x), QS as u32);
        let mut i = 0;
        while i < order {
            let tmp2_qs = smlawb(
                state_qs[i],
                state_qs[i + 1].wrapping_sub(tmp1_qs),
                warping_q16,
            );
            state_qs[i] = tmp1_qs;
            corr_qc[i] += smull(tmp1_qs, state_qs[0]) >> (2 * QS - QC);
            tmp1_qs = smlawb(
                state_qs[i + 1],
                state_qs[i + 2].wrapping_sub(tmp2_qs),
                warping_q16,
            );
            state_qs[i + 1] = tmp2_qs;
            corr_qc[i + 1] += smull(tmp2_qs, state_qs[0]) >> (2 * QS - QC);
            i += 2;
        }
        state_qs[order] = tmp1_qs;
        corr_qc[order] += smull(tmp1_qs, state_qs[0]) >> (2 * QS - QC);
    }
    let lsh = (clz64(corr_qc[0]) - 35).clamp(-12 - QC, 30 - QC);
    for i in 0..=order {
        corr[i] = if lsh >= 0 {
            lshift64(corr_qc[i], lsh as u32) as i32
        } else {
            (corr_qc[i] >> -lsh) as i32
        };
    }
    -(QC + lsh)
}

/// `silk_burg_modified_c`: the order-`d` Burg analysis over `nb_subfr` stacked subframes of
/// `subfr_length` samples each (`d` of them history), the prediction gain limited by
/// `min_inv_gain_q30`; returns the residual energy and its Q.
pub(super) fn burg_modified(
    a_q16: &mut [i32],
    x: &[i16],
    min_inv_gain_q30: i32,
    subfr_length: usize,
    nb_subfr: usize,
    d: usize,
) -> (i32, i32) {
    const QA: i32 = 25;
    const N_BITS_HEAD_ROOM: i32 = 3;
    const MIN_RSHIFTS: i32 = -16;
    const MAX_RSHIFTS: i32 = 32 - QA;
    let cond_fac_q32 = fix_f32(FIND_LPC_COND_FAC, 32);

    let c0_64 = inner_prod16(&x[..subfr_length * nb_subfr], &x[..subfr_length * nb_subfr]);
    let lz = clz64(c0_64);
    let rshifts = (32 + 1 + N_BITS_HEAD_ROOM - lz).clamp(MIN_RSHIFTS, MAX_RSHIFTS);
    let mut c0 = if rshifts > 0 {
        (c0_64 >> rshifts) as i32
    } else {
        lshift(c0_64 as i32, (-rshifts) as u32)
    };
    let mut c_first_row = [0i32; MAX_LPC_ORDER];
    if rshifts > 0 {
        for s in 0..nb_subfr {
            let xp = &x[s * subfr_length..];
            for n in 1..d + 1 {
                c_first_row[n - 1] = c_first_row[n - 1].wrapping_add(
                    (inner_prod16(&xp[..subfr_length - n], &xp[n..subfr_length]) >> rshifts) as i32,
                );
            }
        }
    } else {
        for s in 0..nb_subfr {
            let xp = &x[s * subfr_length..];
            for n in 1..d + 1 {
                let xc = inner_prod_aligned(&xp[..subfr_length - n], &xp[n..subfr_length]);
                c_first_row[n - 1] = c_first_row[n - 1].wrapping_add(lshift(xc, (-rshifts) as u32));
            }
        }
    }
    let mut c_last_row = c_first_row;
    let mut caf = [0i32; MAX_LPC_ORDER + 1];
    let mut cab = [0i32; MAX_LPC_ORDER + 1];
    let mut af_qa = [0i32; MAX_LPC_ORDER];
    caf[0] = c0.wrapping_add(smmul(cond_fac_q32, c0)).wrapping_add(1);
    cab[0] = caf[0];
    let mut inv_gain_q30: i32 = 1 << 30;
    let mut reached_max_gain = false;
    let xi = |s: usize, i: usize| i32::from(x[s * subfr_length + i]);
    for n in 0..d {
        if rshifts > -2 {
            for s in 0..nb_subfr {
                let x1 = lshift(xi(s, n), (16 - rshifts) as u32).wrapping_neg();
                let x2 = lshift(xi(s, subfr_length - n - 1), (16 - rshifts) as u32).wrapping_neg();
                let mut tmp1 = lshift(xi(s, n), (QA - 16) as u32);
                let mut tmp2 = lshift(xi(s, subfr_length - n - 1), (QA - 16) as u32);
                for k in 0..n {
                    c_first_row[k] = smlawb(c_first_row[k], x1, xi(s, n - k - 1));
                    c_last_row[k] = smlawb(c_last_row[k], x2, xi(s, subfr_length - n + k));
                    let atmp_qa = af_qa[k];
                    tmp1 = smlawb(tmp1, atmp_qa, xi(s, n - k - 1));
                    tmp2 = smlawb(tmp2, atmp_qa, xi(s, subfr_length - n + k));
                }
                let tmp1 = lshift(tmp1.wrapping_neg(), (32 - QA - rshifts) as u32);
                let tmp2 = lshift(tmp2.wrapping_neg(), (32 - QA - rshifts) as u32);
                for k in 0..=n {
                    caf[k] = smlawb(caf[k], tmp1, xi(s, n - k));
                    cab[k] = smlawb(cab[k], tmp2, xi(s, subfr_length - n + k - 1));
                }
            }
        } else {
            for s in 0..nb_subfr {
                let x1 = lshift(xi(s, n), (-rshifts) as u32).wrapping_neg();
                let x2 = lshift(xi(s, subfr_length - n - 1), (-rshifts) as u32).wrapping_neg();
                let mut tmp1 = lshift(xi(s, n), 17);
                let mut tmp2 = lshift(xi(s, subfr_length - n - 1), 17);
                for k in 0..n {
                    c_first_row[k] = mla(c_first_row[k], x1, xi(s, n - k - 1));
                    c_last_row[k] = mla(c_last_row[k], x2, xi(s, subfr_length - n + k));
                    let atmp1 = rshift_round(af_qa[k], (QA - 17) as u32);
                    tmp1 = mla(tmp1, xi(s, n - k - 1), atmp1);
                    tmp2 = mla(tmp2, xi(s, subfr_length - n + k), atmp1);
                }
                let tmp1 = tmp1.wrapping_neg();
                let tmp2 = tmp2.wrapping_neg();
                for k in 0..=n {
                    caf[k] = smlaww(caf[k], tmp1, lshift(xi(s, n - k), (-rshifts - 1) as u32));
                    cab[k] = smlaww(
                        cab[k],
                        tmp2,
                        lshift(xi(s, subfr_length - n + k - 1), (-rshifts - 1) as u32),
                    );
                }
            }
        }

        let mut tmp1 = c_first_row[n];
        let mut tmp2 = c_last_row[n];
        let mut num = 0i32;
        let mut nrg = cab[0].wrapping_add(caf[0]);
        for k in 0..n {
            let atmp_qa = af_qa[k];
            let lz = (clz32(atmp_qa.wrapping_abs()) - 1).min(32 - QA);
            let atmp1 = lshift(atmp_qa, lz as u32);
            let sh = (32 - QA - lz) as u32;
            tmp1 = add_lshift32(tmp1, smmul(c_last_row[n - k - 1], atmp1), sh);
            tmp2 = add_lshift32(tmp2, smmul(c_first_row[n - k - 1], atmp1), sh);
            num = add_lshift32(num, smmul(cab[n - k], atmp1), sh);
            nrg = add_lshift32(nrg, smmul(cab[k + 1].wrapping_add(caf[k + 1]), atmp1), sh);
        }
        caf[n + 1] = tmp1;
        cab[n + 1] = tmp2;
        num = num.wrapping_add(tmp2);
        num = lshift(num.wrapping_neg(), 1);

        let mut rc_q31 = if num.wrapping_abs() < nrg {
            div32_varq(num, nrg, 31)
        } else if num > 0 {
            i32::MAX
        } else {
            i32::MIN
        };
        let t = (1 << 30) - smmul(rc_q31, rc_q31);
        let t = lshift(smmul(inv_gain_q30, t), 2);
        if t <= min_inv_gain_q30 {
            let tmp2 = (1 << 30) - div32_varq(min_inv_gain_q30, inv_gain_q30, 30);
            rc_q31 = sqrt_approx(tmp2);
            if rc_q31 > 0 {
                rc_q31 = (rc_q31 + div32(tmp2, rc_q31)) >> 1;
                rc_q31 = lshift(rc_q31, 16);
                if num < 0 {
                    rc_q31 = rc_q31.wrapping_neg();
                }
            }
            inv_gain_q30 = min_inv_gain_q30;
            reached_max_gain = true;
        } else {
            inv_gain_q30 = t;
        }

        for k in 0..(n + 1) >> 1 {
            let t1 = af_qa[k];
            let t2 = af_qa[n - k - 1];
            af_qa[k] = add_lshift32(t1, smmul(t2, rc_q31), 1);
            af_qa[n - k - 1] = add_lshift32(t2, smmul(t1, rc_q31), 1);
        }
        af_qa[n] = rc_q31 >> (31 - QA);

        if reached_max_gain {
            for a in af_qa[n + 1..d].iter_mut() {
                *a = 0;
            }
            break;
        }
        for k in 0..=n + 1 {
            let t1 = caf[k];
            let t2 = cab[n + 1 - k];
            caf[k] = add_lshift32(t1, smmul(t2, rc_q31), 1);
            cab[n + 1 - k] = add_lshift32(t2, smmul(t1, rc_q31), 1);
        }
    }

    if reached_max_gain {
        for k in 0..d {
            a_q16[k] = rshift_round(af_qa[k], (QA - 16) as u32).wrapping_neg();
        }
        if rshifts > 0 {
            for s in 0..nb_subfr {
                let xp = &x[s * subfr_length..][..d];
                c0 = c0.wrapping_sub((inner_prod16(xp, xp) >> rshifts) as i32);
            }
        } else {
            for s in 0..nb_subfr {
                let xp = &x[s * subfr_length..][..d];
                c0 = c0.wrapping_sub(lshift(inner_prod_aligned(xp, xp), (-rshifts) as u32));
            }
        }
        (lshift(smmul(inv_gain_q30, c0), 2), -rshifts)
    } else {
        let mut nrg = caf[0];
        let mut tmp1: i32 = 1 << 16;
        for k in 0..d {
            let atmp1 = rshift_round(af_qa[k], (QA - 16) as u32);
            nrg = smlaww(nrg, caf[k + 1], atmp1);
            tmp1 = smlaww(tmp1, atmp1, atmp1);
            a_q16[k] = atmp1.wrapping_neg();
        }
        (
            smlaww(nrg, smmul(cond_fac_q32, c0), tmp1.wrapping_neg()),
            -rshifts,
        )
    }
}

/// `silk_corrVector_FIX`: `X' * t`, X's columns the `order` lags of `x`.
fn corr_vector(x: &[i16], t: &[i16], l: usize, order: usize, xt: &mut [i32], rshifts: i32) {
    for lag in 0..order {
        let col = &x[order - 1 - lag..][..l];
        xt[lag] = if rshifts > 0 {
            col.iter().zip(&t[..l]).fold(0i32, |acc, (&a, &b)| {
                acc.wrapping_add(smulbb(a.into(), b.into()) >> rshifts)
            })
        } else {
            inner_prod_aligned(col, &t[..l])
        };
    }
}

/// `silk_corrMatrix_FIX`: `X' * X`; returns the energy of `x` and the shift used.
fn corr_matrix(x: &[i16], l: usize, order: usize, xx: &mut [i32]) -> (i32, i32) {
    let (nrg, rshifts) = sum_sqr_shift(&x[..l + order - 1]);
    let sq = |a: i16, b: i16| smulbb(a.into(), b.into()) >> rshifts;
    let mut energy = nrg;
    for i in 0..order - 1 {
        energy = energy.wrapping_sub(sq(x[i], x[i]));
    }
    xx[0] = energy;
    let p1 = order - 1;
    for j in 1..order {
        energy = energy.wrapping_sub(sq(x[p1 + l - j], x[p1 + l - j]));
        energy = energy.wrapping_add(sq(x[p1 - j], x[p1 - j]));
        xx[j * order + j] = energy;
    }
    let mut p2 = order - 2;
    for lag in 1..order {
        let mut energy = if rshifts > 0 {
            (0..l).fold(0i32, |acc, i| acc.wrapping_add(sq(x[p1 + i], x[p2 + i])))
        } else {
            inner_prod_aligned(&x[p1..p1 + l], &x[p2..p2 + l])
        };
        xx[lag * order] = energy;
        xx[lag] = energy;
        for j in 1..order - lag {
            if rshifts > 0 {
                energy = energy.wrapping_sub(sq(x[p1 + l - j], x[p2 + l - j]));
                energy = energy.wrapping_add(sq(x[p1 - j], x[p2 - j]));
            } else {
                energy = energy.wrapping_sub(smulbb(x[p1 + l - j].into(), x[p2 + l - j].into()));
                energy = smlabb(energy, x[p1 - j].into(), x[p2 - j].into());
            }
            xx[(lag + j) * order + j] = energy;
            xx[j * order + lag + j] = energy;
        }
        p2 = p2.wrapping_sub(1);
    }
    (nrg, rshifts)
}

/// `silk_find_LTP_FIX`: each subframe's LTP correlation matrix and vector, normalised, in
/// Q17. `r` is the residual, `r0` the index of the frame's first sample in it.
pub(super) fn find_ltp(
    xx_ltp_q17: &mut [i32; MAX_NB_SUBFR * LTP_ORDER * LTP_ORDER],
    x_x_ltp_q17: &mut [i32; MAX_NB_SUBFR * LTP_ORDER],
    r: &[i16],
    r0: usize,
    lag: &[i32; MAX_NB_SUBFR],
    subfr_length: usize,
    nb_subfr: usize,
) {
    let mut rp = r0;
    for k in 0..nb_subfr {
        let lag_start = rp - (lag[k] as usize + LTP_ORDER / 2);
        let (mut xx, xx_shifts) = sum_sqr_shift(&r[rp..rp + subfr_length + LTP_ORDER]);
        let m = &mut xx_ltp_q17[k * LTP_ORDER * LTP_ORDER..][..LTP_ORDER * LTP_ORDER];
        let (mut nrg, xx_mat_shifts) = corr_matrix(&r[lag_start..], subfr_length, LTP_ORDER, m);
        let extra_shifts = xx_shifts - xx_mat_shifts;
        let x_x_shifts;
        if extra_shifts > 0 {
            x_x_shifts = xx_shifts;
            for v in m.iter_mut() {
                *v >>= extra_shifts;
            }
            nrg >>= extra_shifts;
        } else if extra_shifts < 0 {
            x_x_shifts = xx_mat_shifts;
            xx >>= -extra_shifts;
        } else {
            x_x_shifts = xx_shifts;
        }
        let v = &mut x_x_ltp_q17[k * LTP_ORDER..][..LTP_ORDER];
        corr_vector(
            &r[lag_start..],
            &r[rp..],
            subfr_length,
            LTP_ORDER,
            v,
            x_x_shifts,
        );
        let temp = smlawb(1, nrg, fix_f32(LTP_CORR_INV_MAX, 16)).max(xx);
        for e in m.iter_mut().chain(v.iter_mut()) {
            *e = ((i64::from(*e) << 17) / i64::from(temp)) as i32;
        }
        rp += subfr_length;
    }
}

#[allow(clippy::too_many_arguments)]
/// `silk_residual_energy_FIX`: each subframe's LPC residual energy times its squared gain,
/// with its Q. `x` holds `nb_subfr` subframes, each with `lpc_order` samples of history.
pub(super) fn residual_energy(
    nrgs: &mut [i32; MAX_NB_SUBFR],
    nrgs_q: &mut [i32; MAX_NB_SUBFR],
    x: &[i16],
    a_q12: &[[i16; MAX_LPC_ORDER]; 2],
    gains: &[i32; MAX_NB_SUBFR],
    subfr_length: usize,
    nb_subfr: usize,
    lpc_order: usize,
) {
    let offset = lpc_order + subfr_length;
    let half = MAX_NB_SUBFR >> 1;
    let mut lpc_res = [0i16; (MAX_NB_SUBFR >> 1) * (MAX_LPC_ORDER + 80)];
    for i in 0..nb_subfr >> 1 {
        let n = half * offset;
        lpc_analysis_filter(
            &mut lpc_res[..n],
            &x[i * n..i * n + n],
            &a_q12[i][..lpc_order],
        );
        for j in 0..half {
            let at = lpc_order + j * offset;
            let (e, rshift) = sum_sqr_shift(&lpc_res[at..at + subfr_length]);
            nrgs[i * half + j] = e;
            nrgs_q[i * half + j] = -rshift;
        }
    }
    for i in 0..nb_subfr {
        let lz1 = clz32(nrgs[i]) - 1;
        let lz2 = clz32(gains[i]) - 1;
        let tmp32 = lshift(gains[i], lz2 as u32);
        let tmp32 = smmul(tmp32, tmp32);
        nrgs[i] = smmul(tmp32, lshift(nrgs[i], lz1 as u32));
        nrgs_q[i] += lz1 + 2 * lz2 - 32 - 32;
    }
}

/// `silk_LTP_analysis_filter_FIX`: the LTP residual of each subframe (with `pre_length`
/// samples of history), scaled by its inverse gain. `x0` is the index in `x` of the first
/// output sample.
#[allow(clippy::too_many_arguments)]
pub(super) fn ltp_analysis_filter(
    ltp_res: &mut [i16],
    x: &[i16],
    x0: usize,
    ltp_coef_q14: &[i16; LTP_ORDER * MAX_NB_SUBFR],
    pitch_l: &[i32; MAX_NB_SUBFR],
    inv_gains_q16: &[i32; MAX_NB_SUBFR],
    subfr_length: usize,
    nb_subfr: usize,
    pre_length: usize,
) {
    let mut xp = x0;
    let mut out = 0;
    for k in 0..nb_subfr {
        let b = &ltp_coef_q14[k * LTP_ORDER..][..LTP_ORDER];
        let lag0 = xp - pitch_l[k] as usize;
        for i in 0..subfr_length + pre_length {
            let lagp = lag0 + i;
            let xl = |o: isize| i32::from(x[(lagp as isize + o) as usize]);
            let mut est = smulbb(xl(2), b[0].into());
            est = smlabb(est, xl(1), b[1].into());
            est = smlabb(est, xl(0), b[2].into());
            est = smlabb(est, xl(-1), b[3].into());
            est = smlabb(est, xl(-2), b[4].into());
            let est = rshift_round(est, 14);
            let r = sat16(i32::from(x[xp + i]) - est);
            ltp_res[out + i] = smulwb(inv_gains_q16[k], r.into()) as i16;
        }
        out += subfr_length + pre_length;
        xp += subfr_length;
    }
}

/// `silk_scale_copy_vector16`.
pub(super) fn scale_copy_vector16(out: &mut [i16], input: &[i16], gain_q16: i32) {
    for (o, &x) in out.iter_mut().zip(input) {
        *o = smulwb(gain_q16, x.into()) as i16;
    }
}
