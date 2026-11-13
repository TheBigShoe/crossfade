//! A frame's indices to its parameters (libopus `silk/decode_parameters.c`, `gain_quant.c`,
//! `decode_pitch.c`; RFC 6716 4.2.7.4-4.2.7.6): gains, the LPC filters with NLSF
//! interpolation, pitch lags and the LTP filters.

use super::macros::{limit, log2lin, smulbb, smulwb};
use super::nlsf::{bwexpander, nlsf_decode, nlsf2a};
use super::state::{
    ChannelState, CondCoding, DecoderControl, LTP_ORDER, MAX_LPC_ORDER, MAX_NB_SUBFR, TYPE_VOICED,
};
use super::tables::{
    CB_LAGS_STAGE2, CB_LAGS_STAGE2_10_MS, CB_LAGS_STAGE3, CB_LAGS_STAGE3_10_MS, LTP_VQ_Q7,
    LTPSCALES_TABLE_Q14,
};

const N_LEVELS_QGAIN: i32 = 64;
const MIN_DELTA_GAIN_QUANT: i32 = -4;
const MAX_DELTA_GAIN_QUANT: i32 = 36;
/// `( MIN_QGAIN_DB * 128 ) / 6 + 16 * 128`.
const OFFSET: i32 = (2 * 128) / 6 + 16 * 128;
/// `( 65536 * ( ( ( MAX_QGAIN_DB - MIN_QGAIN_DB ) * 128 ) / 6 ) ) / ( N_LEVELS_QGAIN - 1 )`.
const INV_SCALE_Q16: i32 = (65536 * (((88 - 2) * 128) / 6)) / (N_LEVELS_QGAIN - 1);
const BWE_AFTER_LOSS_Q16: i32 = 63570;
const PE_MIN_LAG_MS: i32 = 2;
const PE_MAX_LAG_MS: i32 = 18;

/// `silk_gains_dequant`.
pub(super) fn gains_dequant(
    gain_q16: &mut [i32; MAX_NB_SUBFR],
    ind: &[i8; MAX_NB_SUBFR],
    prev_ind: &mut i8,
    conditional: bool,
    nb_subfr: usize,
) {
    let mut prev = i32::from(*prev_ind);
    for k in 0..nb_subfr {
        if k == 0 && !conditional {
            // A gain may not fall more than 16 steps (about 21.8 dB) between frames.
            prev = i32::from(ind[k]).max(prev - 16);
        } else {
            let ind_tmp = i32::from(ind[k]) + MIN_DELTA_GAIN_QUANT;
            let threshold = 2 * MAX_DELTA_GAIN_QUANT - N_LEVELS_QGAIN + prev;
            if ind_tmp > threshold {
                prev += (ind_tmp << 1) - threshold;
            } else {
                prev += ind_tmp;
            }
        }
        prev = limit(prev, 0, N_LEVELS_QGAIN - 1);
        gain_q16[k] = log2lin((smulwb(INV_SCALE_Q16, prev) + OFFSET).min(3967));
    }
    *prev_ind = prev as i8;
}

/// `silk_decode_pitch`: the subframes' pitch lags from the lag index and contour.
pub(super) fn decode_pitch(
    lag_index: i16,
    contour_index: i8,
    pitch_lags: &mut [i32; MAX_NB_SUBFR],
    fs_khz: i32,
    nb_subfr: usize,
) {
    let contour = usize::try_from(contour_index).unwrap_or(0);
    let lag_cb = |k: usize| -> i32 {
        // The contour index comes from a table as long as the codebook: always in range.
        let row: &[i8] = match (fs_khz == 8, nb_subfr == MAX_NB_SUBFR) {
            (true, true) => &CB_LAGS_STAGE2[k],
            (true, false) => &CB_LAGS_STAGE2_10_MS[k],
            (false, true) => &CB_LAGS_STAGE3[k],
            (false, false) => &CB_LAGS_STAGE3_10_MS[k],
        };
        row.get(contour).copied().map_or(0, i32::from)
    };
    let min_lag = smulbb(PE_MIN_LAG_MS, fs_khz);
    let max_lag = smulbb(PE_MAX_LAG_MS, fs_khz);
    let lag = min_lag + i32::from(lag_index);
    for (k, p) in pitch_lags.iter_mut().enumerate().take(nb_subfr) {
        *p = limit(lag + lag_cb(k), min_lag, max_lag);
    }
}

/// `silk_decode_parameters`.
pub(super) fn decode_parameters(
    st: &mut ChannelState,
    ctrl: &mut DecoderControl,
    cond_coding: CondCoding,
) {
    let nb_subfr = st.nb_subfr.min(MAX_NB_SUBFR);
    gains_dequant(
        &mut ctrl.gains_q16,
        &st.indices.gains_indices,
        &mut st.last_gain_index,
        cond_coding == CondCoding::Conditionally,
        nb_subfr,
    );

    let order = st.lpc_order;
    let mut nlsf_q15 = [0i16; MAX_LPC_ORDER];
    nlsf_decode(&mut nlsf_q15, &st.indices.nlsf_indices, st.nlsf_cb);
    nlsf2a(&mut ctrl.pred_coef_q12[1][..order], &nlsf_q15[..order]);

    // After a reset there is nothing to interpolate from.
    if st.first_frame_after_reset {
        st.indices.nlsf_interp_coef_q2 = 4;
    }
    let coef = i32::from(st.indices.nlsf_interp_coef_q2);
    if coef < 4 {
        let mut nlsf0_q15 = [0i16; MAX_LPC_ORDER];
        for i in 0..order {
            let prev = i32::from(st.prev_nlsf_q15[i]);
            nlsf0_q15[i] = (prev + ((coef * (i32::from(nlsf_q15[i]) - prev)) >> 2)) as i16;
        }
        nlsf2a(&mut ctrl.pred_coef_q12[0][..order], &nlsf0_q15[..order]);
    } else {
        ctrl.pred_coef_q12[0] = ctrl.pred_coef_q12[1];
    }
    st.prev_nlsf_q15[..order].copy_from_slice(&nlsf_q15[..order]);

    // After a loss, widen the formants a little to hide the seam.
    if st.loss_cnt != 0 {
        bwexpander(&mut ctrl.pred_coef_q12[0][..order], BWE_AFTER_LOSS_Q16);
        bwexpander(&mut ctrl.pred_coef_q12[1][..order], BWE_AFTER_LOSS_Q16);
    }

    if i32::from(st.indices.signal_type) == TYPE_VOICED {
        decode_pitch(
            st.indices.lag_index,
            st.indices.contour_index,
            &mut ctrl.pitch_l,
            st.fs_khz,
            nb_subfr,
        );
        let cbk = LTP_VQ_Q7[st.indices.per_index as usize];
        for k in 0..nb_subfr {
            let row = &cbk[st.indices.ltp_index[k] as usize];
            for (i, &c) in row.iter().enumerate() {
                ctrl.ltp_coef_q14[k * LTP_ORDER + i] = i16::from(c) << 7;
            }
        }
        ctrl.ltp_scale_q14 = i32::from(LTPSCALES_TABLE_Q14[st.indices.ltp_scale_index as usize]);
    } else {
        ctrl.pitch_l[..nb_subfr].fill(0);
        ctrl.ltp_coef_q14[..LTP_ORDER * nb_subfr].fill(0);
        st.indices.per_index = 0;
        ctrl.ltp_scale_q14 = 0;
    }
}
