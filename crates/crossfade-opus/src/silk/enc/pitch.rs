//! The pitch estimator (libopus `silk/fixed/pitch_analysis_core_FIX.c`, with
//! `resampler_down2.c` and `resampler_down2_3.c`): a coarse search at 4 kHz, a refinement
//! with the contour codebook at 8 kHz, and at 12 and 16 kHz a last search on the signal itself.

use super::fix::{
    add_sat32, clz32, div32, div32_varq, fix_f32, fix_f64, inner_prod_aligned,
    insertion_sort_decreasing_i16, lin2log, lshift, rshift_round, sat16, smlawb, smulbb, smulwb,
    sum_sqr_shift,
};
use crate::silk::tables::{
    CB_LAGS_STAGE2, CB_LAGS_STAGE2_10_MS, CB_LAGS_STAGE3, CB_LAGS_STAGE3_10_MS,
};

const PE_MAX_NB_SUBFR: usize = 4;
const PE_SUBFR_LENGTH_MS: usize = 5;
const PE_LTP_MEM_LENGTH_MS: usize = 4 * PE_SUBFR_LENGTH_MS;
const PE_MAX_LAG_MS: i32 = 18;
const PE_MIN_LAG_MS: i32 = 2;
const PE_D_SRCH_LENGTH: usize = 24;
const PE_NB_STAGE3_LAGS: usize = 5;
const PE_NB_CBKS_STAGE2: usize = 3;
const PE_NB_CBKS_STAGE2_EXT: usize = 11;
const PE_NB_CBKS_STAGE3_MAX: usize = 34;
const PE_NB_CBKS_STAGE3_10MS: usize = 12;
const PE_NB_CBKS_STAGE2_10MS: usize = 3;
const PE_SHORTLAG_BIAS: f32 = 0.2;
const PE_PREVLAG_BIAS: f32 = 0.2;
const PE_FLATCONTOUR_BIAS: f32 = 0.05;
pub(super) const SILK_PE_MIN_COMPLEX: usize = 0;

const SCRATCH_SIZE: usize = 22;
const SF_LENGTH_4KHZ: usize = PE_SUBFR_LENGTH_MS * 4;
const SF_LENGTH_8KHZ: usize = PE_SUBFR_LENGTH_MS * 8;
const MIN_LAG_4KHZ: usize = PE_MIN_LAG_MS as usize * 4;
const MIN_LAG_8KHZ: usize = PE_MIN_LAG_MS as usize * 8;
const MAX_LAG_4KHZ: usize = PE_MAX_LAG_MS as usize * 4;
const MAX_LAG_8KHZ: usize = PE_MAX_LAG_MS as usize * 8 - 1;
const CSTRIDE_4KHZ: usize = MAX_LAG_4KHZ + 1 - MIN_LAG_4KHZ;
const CSTRIDE_8KHZ: usize = MAX_LAG_8KHZ + 3 - (MIN_LAG_8KHZ - 2);
const D_COMP_MIN: usize = MIN_LAG_8KHZ - 3;
const D_COMP_MAX: usize = MAX_LAG_8KHZ + 4;
const D_COMP_STRIDE: usize = D_COMP_MAX - D_COMP_MIN;
const MAX_FRAME_LENGTH: usize = (PE_LTP_MEM_LENGTH_MS + PE_MAX_NB_SUBFR * PE_SUBFR_LENGTH_MS) * 16;

static LAG_RANGE_STAGE3: [[[i8; 2]; 4]; 3] = [
    [[-5, 8], [-1, 6], [-1, 6], [-4, 10]],
    [[-6, 10], [-2, 6], [-1, 6], [-5, 10]],
    [[-9, 12], [-3, 7], [-2, 7], [-7, 13]],
];
static LAG_RANGE_STAGE3_10_MS: [[i8; 2]; 2] = [[-3, 7], [-2, 7]];
static NB_CBK_SEARCHS_STAGE3: [usize; 3] = [16, 24, 34];

const RESAMPLER_DOWN2_0: i32 = 9872;
const RESAMPLER_DOWN2_1: i32 = 39809 - 65536;
const RESAMPLER_2_3_COEFS_LQ: [i32; 6] = [-2797, -6507, 4697, 10739, 1567, 8276];

/// `silk_resampler_down2`: halves the rate with two allpass sections.
pub(super) fn resampler_down2(s: &mut [i32; 2], out: &mut [i16], input: &[i16]) {
    for k in 0..input.len() / 2 {
        let in32 = i32::from(input[2 * k]) << 10;
        let y = in32.wrapping_sub(s[0]);
        let x = smlawb(y, y, RESAMPLER_DOWN2_1);
        let mut out32 = s[0].wrapping_add(x);
        s[0] = in32.wrapping_add(x);
        let in32 = i32::from(input[2 * k + 1]) << 10;
        let y = in32.wrapping_sub(s[1]);
        let x = smulwb(y, RESAMPLER_DOWN2_0);
        out32 = out32.wrapping_add(s[1]).wrapping_add(x);
        s[1] = in32.wrapping_add(x);
        out[k] = sat16(rshift_round(out32, 11));
    }
}

/// `silk_resampler_private_AR2`.
pub(super) fn resampler_ar2(s: &mut [i32], out_q8: &mut [i32], input: &[i16], a_q14: &[i32]) {
    for (o, &x) in out_q8.iter_mut().zip(input) {
        let out32 = s[0].wrapping_add(i32::from(x) << 8);
        *o = out32;
        let out32 = lshift(out32, 2);
        s[0] = smlawb(s[1], out32, a_q14[0]);
        s[1] = smulwb(out32, a_q14[1]);
    }
}

/// `silk_resampler_down2_3`: two thirds of the rate, low quality.
fn resampler_down2_3(s: &mut [i32; 6], out: &mut [i16], mut input: &[i16]) {
    const ORDER_FIR: usize = 4;
    const MAX_BATCH: usize = 10 * 48;
    let mut buf = [0i32; MAX_BATCH + ORDER_FIR];
    buf[..ORDER_FIR].copy_from_slice(&s[..ORDER_FIR]);
    let c = &RESAMPLER_2_3_COEFS_LQ;
    let mut o = 0;
    let mut n_in;
    loop {
        n_in = input.len().min(MAX_BATCH);
        let (st, _) = s.split_at_mut(6);
        resampler_ar2(
            &mut st[ORDER_FIR..],
            &mut buf[ORDER_FIR..ORDER_FIR + n_in],
            &input[..n_in],
            c,
        );
        let mut p = 0;
        let mut counter = n_in as i32;
        while counter > 2 {
            let mut r = smulwb(buf[p], c[2]);
            r = smlawb(r, buf[p + 1], c[3]);
            r = smlawb(r, buf[p + 2], c[5]);
            r = smlawb(r, buf[p + 3], c[4]);
            out[o] = sat16(rshift_round(r, 6));
            o += 1;
            let mut r = smulwb(buf[p + 1], c[4]);
            r = smlawb(r, buf[p + 2], c[5]);
            r = smlawb(r, buf[p + 3], c[3]);
            r = smlawb(r, buf[p + 4], c[2]);
            out[o] = sat16(rshift_round(r, 6));
            o += 1;
            p += 3;
            counter -= 3;
        }
        input = &input[n_in..];
        if !input.is_empty() {
            buf.copy_within(n_in..n_in + ORDER_FIR, 0);
        } else {
            break;
        }
    }
    s[..ORDER_FIR].copy_from_slice(&buf[n_in..n_in + ORDER_FIR]);
}

/// `celt_pitch_xcorr`: `xcorr[i]` is `x` against `y[i..]`, `len` samples, wrapping.
fn pitch_xcorr(x: &[i16], y: &[i16], xcorr: &mut [i32], len: usize) {
    for (i, c) in xcorr.iter_mut().enumerate() {
        *c = inner_prod_aligned(&x[..len], &y[i..i + len]);
    }
}

/// What the pitch estimator found for a voiced frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Pitch {
    pub(super) lags: [i32; PE_MAX_NB_SUBFR],
    pub(super) lag_index: i16,
    pub(super) contour_index: i8,
}

/// `silk_pitch_analysis_core`: `Some` for a voiced frame. `frame` holds 20 ms of history and
/// the frame (`(20 + 5 * nb_subfr) * fs_khz` samples); `ltp_corr_q15` is the previous
/// frame's normalised correlation, replaced by this one's (0 when unvoiced).
#[allow(clippy::too_many_arguments)]
pub(super) fn pitch_analysis_core(
    frame_unscaled: &[i16],
    ltp_corr_q15: &mut i32,
    prev_lag: i32,
    search_thres1_q16: i32,
    search_thres2_q13: i32,
    fs_khz: usize,
    complexity: usize,
    nb_subfr: usize,
) -> Option<Pitch> {
    let frame_length = (PE_LTP_MEM_LENGTH_MS + nb_subfr * PE_SUBFR_LENGTH_MS) * fs_khz;
    let frame_length_4khz = (PE_LTP_MEM_LENGTH_MS + nb_subfr * PE_SUBFR_LENGTH_MS) * 4;
    let frame_length_8khz = (PE_LTP_MEM_LENGTH_MS + nb_subfr * PE_SUBFR_LENGTH_MS) * 8;
    let sf_length = PE_SUBFR_LENGTH_MS * fs_khz;
    let min_lag = PE_MIN_LAG_MS * fs_khz as i32;
    let max_lag = PE_MAX_LAG_MS * fs_khz as i32 - 1;

    let (energy, mut shift) = sum_sqr_shift(&frame_unscaled[..frame_length]);
    shift += 3 - clz32(energy);
    let mut frame_scaled = [0i16; MAX_FRAME_LENGTH];
    let frame: &[i16] = if shift > 0 {
        let shift = (shift + 1) >> 1;
        for i in 0..frame_length {
            frame_scaled[i] = frame_unscaled[i] >> shift;
        }
        &frame_scaled[..frame_length]
    } else {
        &frame_unscaled[..frame_length]
    };

    let mut frame_8khz_buf = [0i16; MAX_FRAME_LENGTH / 2];
    let frame_8khz: &[i16] = if fs_khz == 16 {
        let mut st = [0i32; 2];
        resampler_down2(&mut st, &mut frame_8khz_buf, frame);
        &frame_8khz_buf[..frame_length_8khz]
    } else if fs_khz == 12 {
        let mut st = [0i32; 6];
        resampler_down2_3(&mut st, &mut frame_8khz_buf, frame);
        &frame_8khz_buf[..frame_length_8khz]
    } else {
        frame
    };

    let mut frame_4khz = [0i16; MAX_FRAME_LENGTH / 4];
    let mut st = [0i32; 2];
    resampler_down2(&mut st, &mut frame_4khz, &frame_8khz[..frame_length_8khz]);
    for i in (1..frame_length_4khz).rev() {
        frame_4khz[i] = sat16(i32::from(frame_4khz[i]) + i32::from(frame_4khz[i - 1]));
    }

    // First stage, at 4 kHz.
    let mut c = [0i16; PE_MAX_NB_SUBFR * CSTRIDE_8KHZ];
    let mut xcorr32 = [0i32; MAX_LAG_4KHZ - MIN_LAG_4KHZ + 1];
    let mut target = SF_LENGTH_4KHZ << 2;
    for k in 0..nb_subfr >> 1 {
        let tgt = &frame_4khz[target..target + SF_LENGTH_8KHZ];
        pitch_xcorr(
            tgt,
            &frame_4khz[target - MAX_LAG_4KHZ..],
            &mut xcorr32,
            SF_LENGTH_8KHZ,
        );
        let mut basis = target - MIN_LAG_4KHZ;
        let mut cross_corr = xcorr32[MAX_LAG_4KHZ - MIN_LAG_4KHZ];
        let mut normalizer = inner_prod_aligned(tgt, tgt);
        normalizer = normalizer.wrapping_add(inner_prod_aligned(
            &frame_4khz[basis..basis + SF_LENGTH_8KHZ],
            &frame_4khz[basis..basis + SF_LENGTH_8KHZ],
        ));
        normalizer = normalizer.wrapping_add(smulbb(SF_LENGTH_8KHZ as i32, 4000));
        c[k * CSTRIDE_4KHZ] = div32_varq(cross_corr, normalizer, 13 + 1) as i16;
        for d in MIN_LAG_4KHZ + 1..=MAX_LAG_4KHZ {
            basis -= 1;
            cross_corr = xcorr32[MAX_LAG_4KHZ - d];
            let b0 = i32::from(frame_4khz[basis]);
            let bl = i32::from(frame_4khz[basis + SF_LENGTH_8KHZ]);
            normalizer = normalizer.wrapping_add(smulbb(b0, b0) - smulbb(bl, bl));
            c[k * CSTRIDE_4KHZ + d - MIN_LAG_4KHZ] =
                div32_varq(cross_corr, normalizer, 13 + 1) as i16;
        }
        target += SF_LENGTH_8KHZ;
    }

    // Two subframes into one measure, with a bias towards short lags.
    for i in (MIN_LAG_4KHZ..=MAX_LAG_4KHZ).rev() {
        let mut sum = if nb_subfr == PE_MAX_NB_SUBFR {
            i32::from(c[i - MIN_LAG_4KHZ]) + i32::from(c[CSTRIDE_4KHZ + i - MIN_LAG_4KHZ])
        } else {
            i32::from(c[i - MIN_LAG_4KHZ]) << 1
        };
        sum = smlawb(sum, sum, (-(i as i32)) << 4);
        c[i - MIN_LAG_4KHZ] = sum as i16;
    }

    let mut length_d_srch = 4 + 2 * complexity;
    let mut d_srch = [0usize; PE_D_SRCH_LENGTH];
    insertion_sort_decreasing_i16(&mut c[..CSTRIDE_4KHZ], &mut d_srch[..length_d_srch]);

    let cmax = i32::from(c[0]);
    if cmax < fix_f32(0.2, 14) {
        *ltp_corr_q15 = 0;
        return None;
    }
    let threshold = smulwb(search_thres1_q16, cmax);
    for i in 0..length_d_srch {
        if i32::from(c[i]) > threshold {
            d_srch[i] = (d_srch[i] + MIN_LAG_4KHZ) << 1;
        } else {
            length_d_srch = i;
            break;
        }
    }

    let mut d_comp = [0i16; D_COMP_STRIDE];
    for &d in &d_srch[..length_d_srch] {
        d_comp[d - D_COMP_MIN] = 1;
    }
    for i in (MIN_LAG_8KHZ..D_COMP_MAX).rev() {
        d_comp[i - D_COMP_MIN] += d_comp[i - 1 - D_COMP_MIN] + d_comp[i - 2 - D_COMP_MIN];
    }
    length_d_srch = 0;
    for i in MIN_LAG_8KHZ..MAX_LAG_8KHZ + 1 {
        if d_comp[i + 1 - D_COMP_MIN] > 0 {
            d_srch[length_d_srch] = i;
            length_d_srch += 1;
        }
    }
    for i in (MIN_LAG_8KHZ..D_COMP_MAX).rev() {
        d_comp[i - D_COMP_MIN] +=
            d_comp[i - 1 - D_COMP_MIN] + d_comp[i - 2 - D_COMP_MIN] + d_comp[i - 3 - D_COMP_MIN];
    }
    let mut length_d_comp = 0;
    for i in MIN_LAG_8KHZ..D_COMP_MAX {
        if d_comp[i - D_COMP_MIN] > 0 {
            d_comp[length_d_comp] = (i - 2) as i16;
            length_d_comp += 1;
        }
    }

    // Second stage, at 8 kHz, on the lags with high correlation.
    c.fill(0);
    let mut target = PE_LTP_MEM_LENGTH_MS * 8;
    for k in 0..nb_subfr {
        let tgt = &frame_8khz[target..target + SF_LENGTH_8KHZ];
        let energy_target = inner_prod_aligned(tgt, tgt).wrapping_add(1);
        for &d in &d_comp[..length_d_comp] {
            let d = d as usize;
            let basis = &frame_8khz[target - d..target - d + SF_LENGTH_8KHZ];
            let cross_corr = inner_prod_aligned(tgt, basis);
            c[k * CSTRIDE_8KHZ + d - (MIN_LAG_8KHZ - 2)] = if cross_corr > 0 {
                let energy_basis = inner_prod_aligned(basis, basis);
                div32_varq(cross_corr, energy_target.wrapping_add(energy_basis), 13 + 1) as i16
            } else {
                0
            };
        }
        target += SF_LENGTH_8KHZ;
    }

    let mut ccmax = i32::MIN;
    let mut ccmax_b = i32::MIN;
    let mut cbimax = 0;
    let mut lag = -1i32;
    let mut prev_lag = prev_lag;
    let prev_lag_log2_q7 = if prev_lag > 0 {
        if fs_khz == 12 {
            prev_lag = (prev_lag << 1) / 3;
        } else if fs_khz == 16 {
            prev_lag >>= 1;
        }
        lin2log(prev_lag)
    } else {
        0
    };

    // The stage 2 codebook, as `matrix_ptr(Lag_CB_ptr, subframe, vector, cbk_size)`.
    let (cbk2, nb_cbk_search): (&dyn Fn(usize, usize) -> i32, usize) =
        if nb_subfr == PE_MAX_NB_SUBFR {
            let n = if fs_khz == 8 && complexity > SILK_PE_MIN_COMPLEX {
                PE_NB_CBKS_STAGE2_EXT
            } else {
                PE_NB_CBKS_STAGE2
            };
            (&|i, j| i32::from(CB_LAGS_STAGE2[i][j]), n)
        } else {
            (
                &|i, j| i32::from(CB_LAGS_STAGE2_10_MS[i][j]),
                PE_NB_CBKS_STAGE2_10MS,
            )
        };
    let mut cc = [0i32; PE_NB_CBKS_STAGE2_EXT];
    for &d in &d_srch[..length_d_srch] {
        for j in 0..nb_cbk_search {
            cc[j] = 0;
            for i in 0..nb_subfr {
                let d_subfr = d as i32 + cbk2(i, j);
                cc[j] +=
                    i32::from(c[i * CSTRIDE_8KHZ + (d_subfr - (MIN_LAG_8KHZ as i32 - 2)) as usize]);
            }
        }
        let mut ccmax_new = i32::MIN;
        let mut cbimax_new = 0;
        for (i, &v) in cc[..nb_cbk_search].iter().enumerate() {
            if v > ccmax_new {
                ccmax_new = v;
                cbimax_new = i;
            }
        }
        let lag_log2_q7 = lin2log(d as i32);
        let mut ccmax_new_b =
            ccmax_new - (smulbb(nb_subfr as i32 * fix_f32(PE_SHORTLAG_BIAS, 13), lag_log2_q7) >> 7);
        if prev_lag > 0 {
            let mut delta = lag_log2_q7 - prev_lag_log2_q7;
            delta = smulbb(delta, delta) >> 7;
            let mut prev_lag_bias_q13 = smulbb(
                nb_subfr as i32 * fix_f32(PE_PREVLAG_BIAS, 13),
                *ltp_corr_q15,
            ) >> 15;
            prev_lag_bias_q13 = div32(
                prev_lag_bias_q13.wrapping_mul(delta),
                delta + fix_f64(0.5, 7),
            );
            ccmax_new_b -= prev_lag_bias_q13;
        }
        if ccmax_new_b > ccmax_b
            && ccmax_new > smulbb(nb_subfr as i32, search_thres2_q13)
            && i32::from(CB_LAGS_STAGE2[0][cbimax_new]) <= MIN_LAG_8KHZ as i32
        {
            ccmax_b = ccmax_new_b;
            ccmax = ccmax_new;
            lag = d as i32;
            cbimax = cbimax_new;
        }
    }
    if lag == -1 {
        *ltp_corr_q15 = 0;
        return None;
    }
    *ltp_corr_q15 = (ccmax / nb_subfr as i32) << 2;

    let mut lags = [0i32; PE_MAX_NB_SUBFR];
    if fs_khz > 8 {
        let cbimax_old = cbimax;
        lag = if fs_khz == 12 {
            smulbb(lag, 3) >> 1
        } else if fs_khz == 16 {
            lag << 1
        } else {
            smulbb(lag, 3)
        };
        lag = lag.clamp(min_lag, max_lag);
        let start_lag = (lag - 2).max(min_lag);
        let end_lag = (lag + 2).min(max_lag);
        let mut lag_new = lag;
        cbimax = 0;
        ccmax = i32::MIN;
        for k in 0..nb_subfr {
            lags[k] = lag + 2 * i32::from(CB_LAGS_STAGE2[k][cbimax_old]);
        }
        let (cbk3, nb_cbk_search): (&dyn Fn(usize, usize) -> i32, usize) =
            if nb_subfr == PE_MAX_NB_SUBFR {
                (
                    &|k, i| i32::from(CB_LAGS_STAGE3[k][i]),
                    NB_CBK_SEARCHS_STAGE3[complexity],
                )
            } else {
                (
                    &|k, i| i32::from(CB_LAGS_STAGE3_10_MS[k][i]),
                    PE_NB_CBKS_STAGE3_10MS,
                )
            };
        let mut cross_corr_st3 =
            [[0i32; PE_NB_STAGE3_LAGS]; PE_MAX_NB_SUBFR * PE_NB_CBKS_STAGE3_MAX];
        let mut energies_st3 = [[0i32; PE_NB_STAGE3_LAGS]; PE_MAX_NB_SUBFR * PE_NB_CBKS_STAGE3_MAX];
        calc_corr_st3(
            &mut cross_corr_st3,
            frame,
            start_lag,
            sf_length,
            nb_subfr,
            complexity,
        );
        calc_energy_st3(
            &mut energies_st3,
            frame,
            start_lag,
            sf_length,
            nb_subfr,
            complexity,
        );

        let contour_bias_q15 = fix_f32(PE_FLATCONTOUR_BIAS, 15) / lag;
        let tgt = &frame[PE_LTP_MEM_LENGTH_MS * fs_khz..][..nb_subfr * sf_length];
        let energy_target = inner_prod_aligned(tgt, tgt).wrapping_add(1);
        for (lag_counter, d) in (start_lag..=end_lag).enumerate() {
            for j in 0..nb_cbk_search {
                let mut cross_corr = 0i32;
                let mut energy = energy_target;
                for k in 0..nb_subfr {
                    cross_corr =
                        cross_corr.wrapping_add(cross_corr_st3[k * nb_cbk_search + j][lag_counter]);
                    energy = energy.wrapping_add(energies_st3[k * nb_cbk_search + j][lag_counter]);
                }
                let ccmax_new = if cross_corr > 0 {
                    let v = div32_varq(cross_corr, energy, 13 + 1);
                    let diff = i32::from(i16::MAX) - contour_bias_q15 * j as i32;
                    smulwb(v, diff)
                } else {
                    0
                };
                if ccmax_new > ccmax && d + i32::from(CB_LAGS_STAGE3[0][j]) <= max_lag {
                    ccmax = ccmax_new;
                    lag_new = d;
                    cbimax = j;
                }
            }
        }
        for k in 0..nb_subfr {
            lags[k] = (lag_new + cbk3(k, cbimax)).clamp(min_lag, PE_MAX_LAG_MS * fs_khz as i32);
        }
        Some(Pitch {
            lags,
            lag_index: (lag_new - min_lag) as i16,
            contour_index: cbimax as i8,
        })
    } else {
        for k in 0..nb_subfr {
            lags[k] = (lag + cbk2(k, cbimax)).clamp(MIN_LAG_8KHZ as i32, PE_MAX_LAG_MS * 8);
        }
        Some(Pitch {
            lags,
            lag_index: (lag - MIN_LAG_8KHZ as i32) as i16,
            contour_index: cbimax as i8,
        })
    }
}

type Stage3 = [[i32; PE_NB_STAGE3_LAGS]; PE_MAX_NB_SUBFR * PE_NB_CBKS_STAGE3_MAX];

/// The stage 3 tables for a frame size: lag ranges, codebook, codebook vectors searched.
fn stage3_tables(nb_subfr: usize, complexity: usize) -> (&'static [[i8; 2]], usize) {
    if nb_subfr == PE_MAX_NB_SUBFR {
        (
            &LAG_RANGE_STAGE3[complexity],
            NB_CBK_SEARCHS_STAGE3[complexity],
        )
    } else {
        (&LAG_RANGE_STAGE3_10_MS, PE_NB_CBKS_STAGE3_10MS)
    }
}

fn stage3_cb(nb_subfr: usize, k: usize, i: usize) -> i32 {
    if nb_subfr == PE_MAX_NB_SUBFR {
        i32::from(CB_LAGS_STAGE3[k][i])
    } else {
        i32::from(CB_LAGS_STAGE3_10_MS[k][i])
    }
}

/// `silk_P_Ana_calc_corr_st3`.
fn calc_corr_st3(
    out: &mut Stage3,
    frame: &[i16],
    start_lag: i32,
    sf_length: usize,
    nb_subfr: usize,
    complexity: usize,
) {
    let (range, nb_cbk_search) = stage3_tables(nb_subfr, complexity);
    let mut scratch = [0i32; SCRATCH_SIZE];
    let mut xcorr32 = [0i32; SCRATCH_SIZE];
    let mut target = sf_length << 2;
    for k in 0..nb_subfr {
        let lag_low = i32::from(range[k][0]);
        let lag_high = i32::from(range[k][1]);
        let n = (lag_high - lag_low + 1) as usize;
        let from = (target as i32 - start_lag - lag_high) as usize;
        pitch_xcorr(
            &frame[target..],
            &frame[from..],
            &mut xcorr32[..n],
            sf_length,
        );
        for (counter, j) in (lag_low..=lag_high).enumerate() {
            scratch[counter] = xcorr32[(lag_high - j) as usize];
        }
        for i in 0..nb_cbk_search {
            let idx = (stage3_cb(nb_subfr, k, i) - lag_low) as usize;
            out[k * nb_cbk_search + i].copy_from_slice(&scratch[idx..idx + PE_NB_STAGE3_LAGS]);
        }
        target += sf_length;
    }
}

/// `silk_P_Ana_calc_energy_st3`: energies computed recursively.
fn calc_energy_st3(
    out: &mut Stage3,
    frame: &[i16],
    start_lag: i32,
    sf_length: usize,
    nb_subfr: usize,
    complexity: usize,
) {
    let (range, nb_cbk_search) = stage3_tables(nb_subfr, complexity);
    let mut scratch = [0i32; SCRATCH_SIZE];
    let mut target = sf_length << 2;
    for k in 0..nb_subfr {
        let lag_low = i32::from(range[k][0]);
        let basis = (target as i32 - (start_lag + lag_low)) as usize;
        let b = &frame[basis..basis + sf_length];
        let mut energy = inner_prod_aligned(b, b);
        scratch[0] = energy;
        let lag_diff = (i32::from(range[k][1]) - lag_low + 1) as usize;
        for i in 1..lag_diff {
            let out_s = i32::from(frame[basis + sf_length - i]);
            energy = energy.wrapping_sub(smulbb(out_s, out_s));
            let in_s = i32::from(frame[basis - i]);
            energy = add_sat32(energy, smulbb(in_s, in_s));
            scratch[i] = energy;
        }
        for i in 0..nb_cbk_search {
            let idx = (stage3_cb(nb_subfr, k, i) - lag_low) as usize;
            out[k * nb_cbk_search + i].copy_from_slice(&scratch[idx..idx + PE_NB_STAGE3_LAGS]);
        }
        target += sf_length;
    }
}

#[cfg(test)]
#[path = "pitch_tests.rs"]
mod tests;
