//! Gains and LTP filters, quantised (libopus `silk/gain_quant.c`'s encoder half,
//! `quant_LTP_gains.c`, `VQ_WMat_EC.c`).

use super::fix::{
    add_lshift32, add_pos_sat32, fix_f64, lin2log, log2lin, lshift, mla, smlawb, smulbb, smulwb,
};
use super::tables::{LTP_GAIN_BITS_Q5, LTP_VQ_GAIN_Q7};
use crate::silk::state::{LTP_ORDER, MAX_NB_SUBFR};
use crate::silk::tables::LTP_VQ_Q7;

const N_LEVELS_QGAIN: i32 = 64;
const MIN_DELTA_GAIN_QUANT: i32 = -4;
const MAX_DELTA_GAIN_QUANT: i32 = 36;
const OFFSET: i32 = (2 * 128) / 6 + 16 * 128;
const SCALE_Q16: i32 = (65536 * (N_LEVELS_QGAIN - 1)) / (((88 - 2) * 128) / 6);
const INV_SCALE_Q16: i32 = (65536 * (((88 - 2) * 128) / 6)) / (N_LEVELS_QGAIN - 1);
const MAX_SUM_LOG_GAIN_DB: f64 = 250.0;

/// `silk_gains_quant`: gains to indices with hysteresis, the gains replaced by their
/// quantised values; `prev_ind` is the last index of the previous frame.
pub(super) fn gains_quant(
    ind: &mut [i8; MAX_NB_SUBFR],
    gain_q16: &mut [i32; MAX_NB_SUBFR],
    prev_ind: &mut i8,
    conditional: bool,
    nb_subfr: usize,
) {
    let mut prev = i32::from(*prev_ind);
    for k in 0..nb_subfr {
        // The C stores each step in its opus_int8 index.
        let mut i = i32::from(smulwb(SCALE_Q16, lin2log(gain_q16[k]) - OFFSET) as i8);
        if i < prev {
            i += 1;
        }
        i = i.clamp(0, N_LEVELS_QGAIN - 1);
        if k == 0 && !conditional {
            i = i.clamp(
                (prev + MIN_DELTA_GAIN_QUANT).min(N_LEVELS_QGAIN - 1),
                N_LEVELS_QGAIN - 1,
            );
            prev = i;
        } else {
            i -= prev;
            let threshold = 2 * MAX_DELTA_GAIN_QUANT - N_LEVELS_QGAIN + prev;
            if i > threshold {
                i = threshold + ((i - threshold + 1) >> 1);
            }
            i = i.clamp(MIN_DELTA_GAIN_QUANT, MAX_DELTA_GAIN_QUANT);
            if i > threshold {
                prev += lshift(i, 1) - threshold;
                prev = prev.min(N_LEVELS_QGAIN - 1);
            } else {
                prev += i;
            }
            i -= MIN_DELTA_GAIN_QUANT;
        }
        ind[k] = i as i8;
        gain_q16[k] = log2lin((smulwb(INV_SCALE_Q16, prev) + OFFSET).min(3967));
    }
    *prev_ind = prev as i8;
}

/// `silk_gains_ID`: the indices packed into one number.
pub(super) fn gains_id(ind: &[i8; MAX_NB_SUBFR], nb_subfr: usize) -> i32 {
    ind[..nb_subfr]
        .iter()
        .fold(0, |id, &i| add_lshift32(i32::from(i), id, 8))
}

/// What `silk_VQ_WMat_EC` finds: the index, residual energy, rate-distortion and gain.
struct VqChoice {
    ind: i8,
    res_nrg_q15: i32,
    rate_dist_q8: i32,
    gain_q7: i32,
}

/// `silk_VQ_WMat_EC_c`: the entropy-constrained, matrix-weighted VQ of one subframe's five
/// LTP taps.
fn vq_wmat_ec(
    xx_q17: &[i32],
    x_x_q17: &[i32],
    cb_q7: &[[i8; 5]],
    cb_gain_q7: &[u8],
    cl_q5: &[u8],
    subfr_len: i32,
    max_gain_q7: i32,
) -> VqChoice {
    let mut neg_xx_q24 = [0i32; 5];
    for i in 0..5 {
        neg_xx_q24[i] = lshift(x_x_q17[i], 7).wrapping_neg();
    }
    let mut best = VqChoice {
        ind: 0,
        res_nrg_q15: i32::MAX,
        rate_dist_q8: i32::MAX,
        gain_q7: 0,
    };
    let xx = |i: usize| xx_q17[i];
    for (k, row) in cb_q7.iter().enumerate() {
        let r = |i: usize| i32::from(row[i]);
        let gain_tmp_q7 = i32::from(cb_gain_q7[k]);
        let mut sum1_q15 = fix_f64(1.001, 15);
        let penalty = lshift((gain_tmp_q7 - max_gain_q7).max(0), 11);

        let mut sum2 = mla(neg_xx_q24[0], xx(1), r(1));
        sum2 = mla(sum2, xx(2), r(2));
        sum2 = mla(sum2, xx(3), r(3));
        sum2 = mla(sum2, xx(4), r(4));
        sum2 = lshift(sum2, 1);
        sum2 = mla(sum2, xx(0), r(0));
        sum1_q15 = smlawb(sum1_q15, sum2, r(0));

        sum2 = mla(neg_xx_q24[1], xx(7), r(2));
        sum2 = mla(sum2, xx(8), r(3));
        sum2 = mla(sum2, xx(9), r(4));
        sum2 = lshift(sum2, 1);
        sum2 = mla(sum2, xx(6), r(1));
        sum1_q15 = smlawb(sum1_q15, sum2, r(1));

        sum2 = mla(neg_xx_q24[2], xx(13), r(3));
        sum2 = mla(sum2, xx(14), r(4));
        sum2 = lshift(sum2, 1);
        sum2 = mla(sum2, xx(12), r(2));
        sum1_q15 = smlawb(sum1_q15, sum2, r(2));

        sum2 = mla(neg_xx_q24[3], xx(19), r(4));
        sum2 = lshift(sum2, 1);
        sum2 = mla(sum2, xx(18), r(3));
        sum1_q15 = smlawb(sum1_q15, sum2, r(3));

        sum2 = lshift(neg_xx_q24[4], 1);
        sum2 = mla(sum2, xx(24), r(4));
        sum1_q15 = smlawb(sum1_q15, sum2, r(4));

        if sum1_q15 >= 0 {
            let bits_res_q8 = smulbb(
                subfr_len,
                lin2log(sum1_q15.wrapping_add(penalty)) - (15 << 7),
            );
            // Half the code length: libopus finds it slightly better.
            let bits_tot_q8 = add_lshift32(bits_res_q8, i32::from(cl_q5[k]), 3 - 1);
            if bits_tot_q8 <= best.rate_dist_q8 {
                best = VqChoice {
                    ind: k as i8,
                    res_nrg_q15: sum1_q15.wrapping_add(penalty),
                    rate_dist_q8: bits_tot_q8,
                    gain_q7: gain_tmp_q7,
                };
            }
        }
    }
    best
}

/// What `silk_quant_LTP_gains` writes besides the filter.
pub(super) struct LtpChoice {
    pub(super) cbk_index: [i8; MAX_NB_SUBFR],
    pub(super) periodicity_index: i8,
    pub(super) pred_gain_db_q7: i32,
}

/// `silk_quant_LTP_gains`: the subframes' LTP filters from their correlations, the codebook
/// with the best rate-distortion; `sum_log_gain_q7` carries the cumulative gain limit.
pub(super) fn quant_ltp_gains(
    b_q14: &mut [i16; MAX_NB_SUBFR * LTP_ORDER],
    sum_log_gain_q7: &mut i32,
    xx_q17: &[i32],
    x_x_q17: &[i32],
    subfr_len: usize,
    nb_subfr: usize,
) -> LtpChoice {
    let gain_safety = fix_f64(0.4, 7);
    let mut min_rate_dist_q7 = i32::MAX;
    let mut best_sum_log_gain_q7 = 0;
    let mut out = LtpChoice {
        cbk_index: [0; MAX_NB_SUBFR],
        periodicity_index: 0,
        pred_gain_db_q7: 0,
    };
    let mut best_res_nrg_q15 = 0;
    for k in 0..3 {
        let mut temp_idx = [0i8; MAX_NB_SUBFR];
        let mut res_nrg_q15 = 0i32;
        let mut rate_dist_q7 = 0i32;
        let mut sum_log_gain_tmp_q7 = *sum_log_gain_q7;
        for j in 0..nb_subfr {
            let max_gain_q7 = log2lin(
                (fix_f64(MAX_SUM_LOG_GAIN_DB / 6.0, 7) - sum_log_gain_tmp_q7) + fix_f64(7.0, 7),
            ) - gain_safety;
            let c = vq_wmat_ec(
                &xx_q17[j * LTP_ORDER * LTP_ORDER..][..LTP_ORDER * LTP_ORDER],
                &x_x_q17[j * LTP_ORDER..][..LTP_ORDER],
                LTP_VQ_Q7[k],
                LTP_VQ_GAIN_Q7[k],
                LTP_GAIN_BITS_Q5[k],
                subfr_len as i32,
                max_gain_q7,
            );
            temp_idx[j] = c.ind;
            res_nrg_q15 = add_pos_sat32(res_nrg_q15, c.res_nrg_q15);
            rate_dist_q7 = add_pos_sat32(rate_dist_q7, c.rate_dist_q8);
            sum_log_gain_tmp_q7 =
                (sum_log_gain_tmp_q7 + lin2log(gain_safety + c.gain_q7) - fix_f64(7.0, 7)).max(0);
        }
        if rate_dist_q7 <= min_rate_dist_q7 {
            min_rate_dist_q7 = rate_dist_q7;
            out.periodicity_index = k as i8;
            out.cbk_index = temp_idx;
            best_sum_log_gain_q7 = sum_log_gain_tmp_q7;
        }
        // The C keeps the last codebook's energy, not the chosen one's.
        best_res_nrg_q15 = res_nrg_q15;
    }
    let cb = LTP_VQ_Q7[out.periodicity_index as usize];
    for j in 0..nb_subfr {
        for k in 0..LTP_ORDER {
            b_q14[j * LTP_ORDER + k] =
                lshift(i32::from(cb[out.cbk_index[j] as usize][k]), 7) as i16;
        }
    }
    let res_nrg_q15 = if nb_subfr == 2 {
        best_res_nrg_q15 >> 1
    } else {
        best_res_nrg_q15 >> 2
    };
    *sum_log_gain_q7 = best_sum_log_gain_q7;
    out.pred_gain_db_q7 = smulbb(-3, lin2log(res_nrg_q15) - (15 << 7));
    out
}

#[cfg(test)]
#[path = "quant_tests.rs"]
mod tests;
