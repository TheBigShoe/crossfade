//! A frame's analysis (libopus `silk/fixed/find_pitch_lags_FIX.c`, `find_pred_coefs_FIX.c`,
//! `find_LPC_FIX.c`, `LTP_scale_ctrl_FIX.c`, `process_gains_FIX.c`, and `silk/process_NLSFs.c`):
//! pitch, then (after the noise shaping analysis) the LTP and LPC filters, quantised, and the
//! gains, quantised.

use super::fix::{
    add_sat32, div32, div32_16, div32_varq, fix_f32, fix_f64, log2lin, lshift, lshift_sat32,
    rshift_round, sat16, sigm_q15, smlabb, smlawb, smlaww, smmul, smulbb, smulwb, smulww,
    sqrt_approx, sum_sqr_shift,
};
use super::nlsf_enc::{a2nlsf, interpolate, nlsf_encode, nlsf_vq_weights_laroia};
use super::pitch::pitch_analysis_core;
use super::quant::{gains_quant, quant_ltp_gains};
use super::sigproc::{
    apply_sine_window, autocorr, burg_modified, find_ltp, k2a, ltp_analysis_filter,
    residual_energy, scale_copy_vector16, schur,
};
use super::state::{ChannelEncoder, EncoderControl};
use crate::silk::decode_core::lpc_analysis_filter;
use crate::silk::nlsf::{bwexpander, nlsf2a};
use crate::silk::state::{
    CondCoding, LTP_ORDER, MAX_FRAME_LENGTH, MAX_LPC_ORDER, MAX_NB_SUBFR, TYPE_NO_VOICE_ACTIVITY,
    TYPE_VOICED,
};
use crate::silk::tables::{LTPSCALES_TABLE_Q14, QUANTIZATION_OFFSETS_Q10};

const MAX_FIND_PITCH_LPC_ORDER: usize = 16;
const FIND_PITCH_WHITE_NOISE_FRACTION: f32 = 1e-3;
const FIND_PITCH_BANDWIDTH_EXPANSION: f32 = 0.99;
const MAX_PREDICTION_POWER_GAIN: f32 = 1e4;
const MAX_PREDICTION_POWER_GAIN_AFTER_RESET: f32 = 1e2;
const LAMBDA_OFFSET: f32 = 1.2;
const LAMBDA_SPEECH_ACT: f32 = -0.2;
const LAMBDA_DELAYED_DECISIONS: f32 = -0.05;
const LAMBDA_INPUT_QUALITY: f32 = -0.1;
const LAMBDA_CODING_QUALITY: f32 = -0.2;
const LAMBDA_QUANT_OFFSET: f32 = 0.8;

/// `silk_find_pitch_lags_FIX`: the whitened residual `res` of `x[x0 - ltp_mem_length..]`
/// (the LTP memory, the frame and the pitch look-ahead), and the pitch.
pub(super) fn find_pitch_lags(
    st: &mut ChannelEncoder,
    ctrl: &mut EncoderControl,
    res: &mut [i16],
    x: &[i16],
) {
    let buf_len = st.la_pitch + st.frame_length + st.ltp_mem_length;
    let win_len = st.pitch_lpc_win_length;
    let mut wsig = [0i16; 24 * 16];
    let mut xp = buf_len - win_len;
    apply_sine_window(&mut wsig, &x[xp..], 1, st.la_pitch);
    let mid = win_len - (st.la_pitch << 1);
    wsig[st.la_pitch..st.la_pitch + mid].copy_from_slice(&x[xp + st.la_pitch..][..mid]);
    xp += st.la_pitch + mid;
    apply_sine_window(&mut wsig[st.la_pitch + mid..], &x[xp..], 2, st.la_pitch);

    let order = st.pitch_estimation_lpc_order;
    let mut auto_corr = [0i32; MAX_FIND_PITCH_LPC_ORDER + 1];
    autocorr(&mut auto_corr[..order + 1], &wsig[..win_len]);
    auto_corr[0] = smlawb(
        auto_corr[0],
        auto_corr[0],
        fix_f32(FIND_PITCH_WHITE_NOISE_FRACTION, 16),
    )
    .wrapping_add(1);
    let mut rc_q15 = [0i16; MAX_FIND_PITCH_LPC_ORDER];
    let res_nrg = schur(&mut rc_q15, &auto_corr, order);
    ctrl.pred_gain_q16 = div32_varq(auto_corr[0], res_nrg.max(1), 16);
    let mut a_q24 = [0i32; MAX_FIND_PITCH_LPC_ORDER];
    k2a(&mut a_q24, &rc_q15[..order]);
    let mut a_q12 = [0i16; MAX_FIND_PITCH_LPC_ORDER];
    for i in 0..order {
        a_q12[i] = sat16(a_q24[i] >> 12);
    }
    bwexpander(
        &mut a_q12[..order],
        fix_f32(FIND_PITCH_BANDWIDTH_EXPANSION, 16),
    );
    lpc_analysis_filter(&mut res[..buf_len], &x[..buf_len], &a_q12[..order]);

    if i32::from(st.indices.signal_type) != TYPE_NO_VOICE_ACTIVITY && !st.first_frame_after_reset {
        let mut thrhld_q13 = fix_f64(0.6, 13);
        thrhld_q13 = smlabb(thrhld_q13, fix_f64(-0.004, 13), order as i32);
        thrhld_q13 = smlawb(thrhld_q13, fix_f64(-0.1, 21), st.speech_activity_q8);
        thrhld_q13 = smlabb(thrhld_q13, fix_f64(-0.15, 13), st.prev_signal_type >> 1);
        thrhld_q13 = smlawb(thrhld_q13, fix_f64(-0.1, 14), st.input_tilt_q15);
        let thrhld_q13 = i32::from(sat16(thrhld_q13));
        match pitch_analysis_core(
            &res[..buf_len],
            &mut st.ltp_corr_q15,
            st.prev_lag,
            st.pitch_estimation_threshold_q16,
            thrhld_q13,
            st.fs_khz as usize,
            st.pitch_estimation_complexity,
            st.nb_subfr,
        ) {
            Some(p) => {
                ctrl.pitch_l = p.lags;
                st.indices.lag_index = p.lag_index;
                st.indices.contour_index = p.contour_index;
                st.indices.signal_type = TYPE_VOICED as i8;
            }
            None => {
                ctrl.pitch_l = [0; MAX_NB_SUBFR];
                st.indices.lag_index = 0;
                st.indices.contour_index = 0;
                st.indices.signal_type = super::state::TYPE_UNVOICED as i8;
            }
        }
    } else {
        ctrl.pitch_l = [0; MAX_NB_SUBFR];
        st.indices.lag_index = 0;
        st.indices.contour_index = 0;
        st.ltp_corr_q15 = 0;
    }
}

/// `silk_find_pred_coefs_FIX`: the LTP filter (voiced), the LPC filter and its NLSFs,
/// quantised, and the residual energies. `res_pitch` and `x` are indexed by `x0`, the
/// frame's first sample.
pub(super) fn find_pred_coefs(
    st: &mut ChannelEncoder,
    ctrl: &mut EncoderControl,
    res_pitch: &[i16],
    x: &[i16],
    x0: usize,
    cond_coding: CondCoding,
) {
    let nb_subfr = st.nb_subfr;
    let order = st.predict_lpc_order;
    let mut inv_gains_q16 = [0i32; MAX_NB_SUBFR];
    let mut local_gains = [0i32; MAX_NB_SUBFR];
    let mut min_gain_q16 = i32::MAX >> 6;
    for &g in &ctrl.gains_q16[..nb_subfr] {
        min_gain_q16 = min_gain_q16.min(g);
    }
    for i in 0..nb_subfr {
        inv_gains_q16[i] = div32_varq(min_gain_q16, ctrl.gains_q16[i], 16 - 2).max(100);
        local_gains[i] = div32(1 << 16, inv_gains_q16[i]);
    }

    let mut lpc_in_pre = [0i16; MAX_NB_SUBFR * MAX_LPC_ORDER + MAX_FRAME_LENGTH];
    if i32::from(st.indices.signal_type) == TYPE_VOICED {
        let mut xx_ltp = [0i32; MAX_NB_SUBFR * LTP_ORDER * LTP_ORDER];
        let mut x_x_ltp = [0i32; MAX_NB_SUBFR * LTP_ORDER];
        find_ltp(
            &mut xx_ltp,
            &mut x_x_ltp,
            res_pitch,
            x0,
            &ctrl.pitch_l,
            st.subfr_length,
            nb_subfr,
        );
        let c = quant_ltp_gains(
            &mut ctrl.ltp_coef_q14,
            &mut st.sum_log_gain_q7,
            &xx_ltp,
            &x_x_ltp,
            st.subfr_length,
            nb_subfr,
        );
        st.indices.ltp_index = c.cbk_index;
        st.indices.per_index = c.periodicity_index;
        ctrl.lt_pred_cod_gain_q7 = c.pred_gain_db_q7;
        ltp_scale_ctrl(st, ctrl, cond_coding);
        ltp_analysis_filter(
            &mut lpc_in_pre,
            x,
            x0 - order,
            &ctrl.ltp_coef_q14,
            &ctrl.pitch_l,
            &inv_gains_q16,
            st.subfr_length,
            nb_subfr,
            order,
        );
    } else {
        let mut xp = x0 - order;
        let mut out = 0;
        let n = st.subfr_length + order;
        for i in 0..nb_subfr {
            scale_copy_vector16(
                &mut lpc_in_pre[out..out + n],
                &x[xp..xp + n],
                inv_gains_q16[i],
            );
            out += n;
            xp += st.subfr_length;
        }
        ctrl.ltp_coef_q14[..nb_subfr * LTP_ORDER].fill(0);
        ctrl.lt_pred_cod_gain_q7 = 0;
        st.sum_log_gain_q7 = 0;
    }

    let min_inv_gain_q30 = if st.first_frame_after_reset {
        fix_f32(1.0 / MAX_PREDICTION_POWER_GAIN_AFTER_RESET, 30)
    } else {
        let g = log2lin(smlawb(
            16 << 7,
            ctrl.lt_pred_cod_gain_q7,
            fix_f64(1.0 / 3.0, 16),
        ));
        div32_varq(
            g,
            smulww(
                fix_f32(MAX_PREDICTION_POWER_GAIN, 0),
                smlawb(
                    fix_f64(0.25, 18),
                    fix_f64(0.75, 18),
                    ctrl.coding_quality_q14,
                ),
            ),
            14,
        )
    };

    let mut nlsf_q15 = [0i16; MAX_LPC_ORDER];
    let n = nb_subfr * (st.subfr_length + order);
    find_lpc(st, &mut nlsf_q15, &lpc_in_pre[..n], min_inv_gain_q30);
    process_nlsfs(st, &mut ctrl.pred_coef_q12, &mut nlsf_q15);
    residual_energy(
        &mut ctrl.res_nrg,
        &mut ctrl.res_nrg_q,
        &lpc_in_pre,
        &ctrl.pred_coef_q12,
        &local_gains,
        st.subfr_length,
        nb_subfr,
        order,
    );
    st.prev_nlsfq_q15 = nlsf_q15;
}

/// `silk_find_LPC_FIX`: the NLSFs of the frame's LPC filter (Burg), and whether the first
/// half is better coded by interpolating from the previous frame's.
fn find_lpc(
    st: &mut ChannelEncoder,
    nlsf_q15: &mut [i16; MAX_LPC_ORDER],
    x: &[i16],
    min_inv_gain_q30: i32,
) {
    let order = st.predict_lpc_order;
    let subfr_length = st.subfr_length + order;
    st.indices.nlsf_interp_coef_q2 = 4;
    let mut a_q16 = [0i32; MAX_LPC_ORDER];
    let (mut res_nrg, mut res_nrg_q) = burg_modified(
        &mut a_q16,
        x,
        min_inv_gain_q30,
        subfr_length,
        st.nb_subfr,
        order,
    );

    if st.use_interpolated_nlsfs && !st.first_frame_after_reset && st.nb_subfr == MAX_NB_SUBFR {
        let mut a_tmp_q16 = [0i32; MAX_LPC_ORDER];
        let (res_tmp_nrg, res_tmp_nrg_q) = burg_modified(
            &mut a_tmp_q16,
            &x[2 * subfr_length..],
            min_inv_gain_q30,
            subfr_length,
            2,
            order,
        );
        let shift = res_tmp_nrg_q - res_nrg_q;
        if shift >= 0 {
            if shift < 32 {
                res_nrg = res_nrg.wrapping_sub(res_tmp_nrg >> shift);
            }
        } else {
            res_nrg = (res_nrg >> -shift).wrapping_sub(res_tmp_nrg);
            res_nrg_q = res_tmp_nrg_q;
        }
        a2nlsf(&mut nlsf_q15[..order], &mut a_tmp_q16[..order]);

        let mut lpc_res = [0i16; 2 * (80 + MAX_LPC_ORDER)];
        let mut nlsf0_q15 = [0i16; MAX_LPC_ORDER];
        let mut a_tmp_q12 = [0i16; MAX_LPC_ORDER];
        for k in (0..4).rev() {
            interpolate(
                &mut nlsf0_q15[..order],
                &st.prev_nlsfq_q15[..order],
                &nlsf_q15[..order],
                k,
            );
            nlsf2a(&mut a_tmp_q12[..order], &nlsf0_q15[..order]);
            lpc_analysis_filter(
                &mut lpc_res[..2 * subfr_length],
                &x[..2 * subfr_length],
                &a_tmp_q12[..order],
            );
            let n = subfr_length - order;
            let (mut res_nrg0, rshift0) = sum_sqr_shift(&lpc_res[order..order + n]);
            let (mut res_nrg1, rshift1) =
                sum_sqr_shift(&lpc_res[order + subfr_length..order + subfr_length + n]);
            let shift = rshift0 - rshift1;
            let res_nrg_interp_q = if shift >= 0 {
                res_nrg1 >>= shift;
                -rshift0
            } else {
                res_nrg0 >>= -shift;
                -rshift1
            };
            let res_nrg_interp = res_nrg0.wrapping_add(res_nrg1);
            let shift = res_nrg_interp_q - res_nrg_q;
            let is_interp_lower = if shift >= 0 {
                (res_nrg_interp >> shift) < res_nrg
            } else if -shift < 32 {
                res_nrg_interp < (res_nrg >> -shift)
            } else {
                false
            };
            if is_interp_lower {
                res_nrg = res_nrg_interp;
                res_nrg_q = res_nrg_interp_q;
                st.indices.nlsf_interp_coef_q2 = k as i8;
            }
        }
    }
    if st.indices.nlsf_interp_coef_q2 == 4 {
        a2nlsf(&mut nlsf_q15[..order], &mut a_q16[..order]);
    }
}

/// `silk_process_NLSFs`: quantises the NLSFs (in place) and gives the frame's two LPC
/// filters, the first interpolated when the frame says so.
fn process_nlsfs(
    st: &mut ChannelEncoder,
    pred_coef_q12: &mut [[i16; MAX_LPC_ORDER]; 2],
    nlsf_q15: &mut [i16; MAX_LPC_ORDER],
) {
    let order = st.predict_lpc_order;
    let mut nlsf_mu_q20 = smlawb(
        fix_f64(0.003, 20),
        fix_f64(-0.001, 28),
        st.speech_activity_q8,
    );
    if st.nb_subfr == 2 {
        nlsf_mu_q20 += nlsf_mu_q20 >> 1;
    }
    let mut w_qw = [0i16; MAX_LPC_ORDER];
    nlsf_vq_weights_laroia(&mut w_qw[..order], &nlsf_q15[..order]);
    let do_interpolate = st.use_interpolated_nlsfs && st.indices.nlsf_interp_coef_q2 < 4;
    let mut nlsf0_temp_q15 = [0i16; MAX_LPC_ORDER];
    if do_interpolate {
        let ifact = i32::from(st.indices.nlsf_interp_coef_q2);
        interpolate(
            &mut nlsf0_temp_q15[..order],
            &st.prev_nlsfq_q15[..order],
            &nlsf_q15[..order],
            ifact,
        );
        let mut w0_temp = [0i16; MAX_LPC_ORDER];
        nlsf_vq_weights_laroia(&mut w0_temp[..order], &nlsf0_temp_q15[..order]);
        let i_sqr_q15 = lshift(smulbb(ifact, ifact), 11);
        for i in 0..order {
            w_qw[i] =
                ((i32::from(w_qw[i]) >> 1) + (smulbb(w0_temp[i].into(), i_sqr_q15) >> 16)) as i16;
        }
    }
    nlsf_encode(
        &mut st.indices.nlsf_indices,
        nlsf_q15,
        st.nlsf_cb,
        &w_qw[..order],
        nlsf_mu_q20,
        st.nlsf_msvq_survivors,
        st.indices.signal_type.into(),
    );
    nlsf2a(&mut pred_coef_q12[1][..order], &nlsf_q15[..order]);
    if do_interpolate {
        let ifact = i32::from(st.indices.nlsf_interp_coef_q2);
        interpolate(
            &mut nlsf0_temp_q15[..order],
            &st.prev_nlsfq_q15[..order],
            &nlsf_q15[..order],
            ifact,
        );
        nlsf2a(&mut pred_coef_q12[0][..order], &nlsf0_temp_q15[..order]);
    } else {
        let [first, second] = pred_coef_q12;
        first[..order].copy_from_slice(&second[..order]);
    }
}

/// `silk_LTP_scale_ctrl_FIX`: how much to scale the LTP state at a packet's first frame,
/// against loss.
fn ltp_scale_ctrl(st: &mut ChannelEncoder, ctrl: &mut EncoderControl, cond_coding: CondCoding) {
    if cond_coding == CondCoding::Independently {
        let mut round_loss = st.packet_loss_perc * st.n_frames_per_packet as i32;
        if st.lbrr_flag {
            round_loss = 2 + smulbb(round_loss, round_loss) / 100;
        }
        let g = smulbb(ctrl.lt_pred_cod_gain_q7, round_loss);
        let mut ix = i8::from(g > log2lin(128 * 7 + 2900 - st.snr_db_q7));
        ix += i8::from(g > log2lin(128 * 7 + 3900 - st.snr_db_q7));
        st.indices.ltp_scale_index = ix;
    } else {
        st.indices.ltp_scale_index = 0;
    }
    ctrl.ltp_scale_q14 = i32::from(LTPSCALES_TABLE_Q14[st.indices.ltp_scale_index as usize]);
}

/// `silk_process_gains_FIX`: the gains adjusted for LTP gain and residual energy, then
/// quantised; the voiced quantiser offset and the rate-distortion trade-off `lambda_q10`.
pub(super) fn process_gains(
    st: &mut ChannelEncoder,
    ctrl: &mut EncoderControl,
    cond_coding: CondCoding,
) {
    let nb_subfr = st.nb_subfr;
    if i32::from(st.indices.signal_type) == TYPE_VOICED {
        let s_q16 = -sigm_q15(rshift_round(ctrl.lt_pred_cod_gain_q7 - fix_f64(12.0, 7), 4));
        for g in ctrl.gains_q16[..nb_subfr].iter_mut() {
            *g = smlawb(*g, *g, s_q16);
        }
    }
    let inv_max_sqr_val_q16 = div32_16(
        log2lin(smulwb(
            fix_f64(21.0 + 16.0 / 0.33, 7) - st.snr_db_q7,
            fix_f64(0.33, 16),
        )),
        st.subfr_length as i32,
    );
    for k in 0..nb_subfr {
        let res_nrg = ctrl.res_nrg[k];
        let mut res_nrg_part = smulww(res_nrg, inv_max_sqr_val_q16);
        let q = ctrl.res_nrg_q[k];
        if q > 0 {
            res_nrg_part = rshift_round(res_nrg_part, q as u32);
        } else if res_nrg_part >= (i32::MAX >> -q) {
            res_nrg_part = i32::MAX;
        } else {
            res_nrg_part = lshift(res_nrg_part, (-q) as u32);
        }
        let gain = ctrl.gains_q16[k];
        let gain_squared = add_sat32(res_nrg_part, smmul(gain, gain));
        if gain_squared < i32::from(i16::MAX) {
            let gain_squared = smlaww(lshift(res_nrg_part, 16), gain, gain);
            let gain = sqrt_approx(gain_squared).min(i32::MAX >> 8);
            ctrl.gains_q16[k] = lshift_sat32(gain, 8);
        } else {
            let gain = sqrt_approx(gain_squared).min(i32::MAX >> 16);
            ctrl.gains_q16[k] = lshift_sat32(gain, 16);
        }
    }
    ctrl.gains_unq_q16 = ctrl.gains_q16;
    ctrl.last_gain_index_prev = st.s_shape.last_gain_index;
    gains_quant(
        &mut st.indices.gains_indices,
        &mut ctrl.gains_q16,
        &mut st.s_shape.last_gain_index,
        cond_coding == CondCoding::Conditionally,
        nb_subfr,
    );
    if i32::from(st.indices.signal_type) == TYPE_VOICED {
        st.indices.quant_offset_type =
            if ctrl.lt_pred_cod_gain_q7 + (st.input_tilt_q15 >> 8) > fix_f64(1.0, 7) {
                0
            } else {
                1
            };
    }
    let quant_offset_q10 = i32::from(
        QUANTIZATION_OFFSETS_Q10[(st.indices.signal_type >> 1) as usize]
            [st.indices.quant_offset_type as usize],
    );
    ctrl.lambda_q10 = fix_f32(LAMBDA_OFFSET, 10)
        + smulbb(
            fix_f32(LAMBDA_DELAYED_DECISIONS, 10),
            st.n_states_delayed_decision as i32,
        )
        + smulwb(fix_f32(LAMBDA_SPEECH_ACT, 18), st.speech_activity_q8)
        + smulwb(fix_f32(LAMBDA_INPUT_QUALITY, 12), ctrl.input_quality_q14)
        + smulwb(fix_f32(LAMBDA_CODING_QUALITY, 12), ctrl.coding_quality_q14)
        + smulwb(fix_f32(LAMBDA_QUANT_OFFSET, 16), quant_offset_q10);
}
