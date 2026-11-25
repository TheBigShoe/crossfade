//! The noise-shaping quantiser (libopus `silk/NSQ.c`, `NSQ.h`): the excitation's pulses,
//! chosen one sample at a time, while running the decoder's own LTP and LPC synthesis, so
//! that `xq` is the decoder's output sample for sample.

use super::fix::{
    add_lshift32, add_sat32, div32_varq, inverse32_varq, lshift, rshift_round, sat16, silk_rand,
    smlabb, smlawb, smlawt, smulbb, smulwb, smulww, sub_lshift32,
};
use super::sigproc::MAX_SHAPE_LPC_ORDER;
use super::state::{NSQ_LPC_BUF_LENGTH, NsqState};
use crate::silk::decode_core::lpc_analysis_filter;
use crate::silk::state::{LTP_ORDER, MAX_FRAME_LENGTH, MAX_LPC_ORDER, MAX_NB_SUBFR, TYPE_VOICED};
use crate::silk::tables::QUANTIZATION_OFFSETS_Q10;

pub(super) const HARM_SHAPE_FIR_TAPS: usize = 3;
pub(super) const QUANT_LEVEL_ADJUST_Q10: i32 = 80;

/// What the quantisers read of the encoder's state and control (`psEncC` and the frame's
/// parameters in `silk_NSQ`'s argument list).
pub(super) struct NsqParams<'a> {
    pub(super) ltp_mem_length: usize,
    pub(super) frame_length: usize,
    pub(super) subfr_length: usize,
    pub(super) nb_subfr: usize,
    pub(super) predict_lpc_order: usize,
    pub(super) shaping_lpc_order: usize,
    pub(super) warping_q16: i32,
    pub(super) n_states_delayed_decision: usize,
    pub(super) signal_type: i32,
    pub(super) quant_offset_type: i32,
    pub(super) nlsf_interp_coef_q2: i32,
    pub(super) pred_coef_q12: &'a [[i16; MAX_LPC_ORDER]; 2],
    pub(super) ltp_coef_q14: &'a [i16; LTP_ORDER * MAX_NB_SUBFR],
    pub(super) ar_q13: &'a [i16; MAX_NB_SUBFR * MAX_SHAPE_LPC_ORDER],
    pub(super) harm_shape_gain_q14: &'a [i32; MAX_NB_SUBFR],
    pub(super) tilt_q14: &'a [i32; MAX_NB_SUBFR],
    pub(super) lf_shp_q14: &'a [i32; MAX_NB_SUBFR],
    pub(super) gains_q16: &'a [i32; MAX_NB_SUBFR],
    pub(super) pitch_l: &'a [i32; MAX_NB_SUBFR],
    pub(super) lambda_q10: i32,
    pub(super) ltp_scale_q14: i32,
}

/// `silk_noise_shape_quantizer_short_prediction_c`: the LPC prediction from `buf[at]`
/// backwards, with the rounding bias.
#[inline(always)]
pub(super) fn short_prediction(buf: &[i32], at: usize, coef: &[i16], order: usize) -> i32 {
    let mut out = (order >> 1) as i32;
    for j in 0..order {
        out = smlawb(out, buf[at - j], coef[j].into());
    }
    out
}

/// `silk_NSQ_c`: quantises the frame `x16`, writing `pulses`; `seed` is the frame's seed index.
pub(super) fn nsq(
    p: &NsqParams<'_>,
    st: &mut NsqState,
    seed: i8,
    x16: &[i16],
    pulses: &mut [i8; MAX_FRAME_LENGTH],
) {
    st.rand_seed = i32::from(seed);
    let mut lag = st.lag_prev;
    let offset_q10 = i32::from(
        QUANTIZATION_OFFSETS_Q10[(p.signal_type >> 1) as usize][p.quant_offset_type as usize],
    );
    let lsf_interpolation = p.nlsf_interp_coef_q2 != 4;

    let mut s_ltp_q15 = [0i32; 2 * MAX_FRAME_LENGTH];
    let mut s_ltp = [0i16; 2 * MAX_FRAME_LENGTH];
    let mut x_sc_q10 = [0i32; MAX_FRAME_LENGTH / MAX_NB_SUBFR];
    st.s_ltp_shp_buf_idx = p.ltp_mem_length;
    st.s_ltp_buf_idx = p.ltp_mem_length;
    let mut pxq = p.ltp_mem_length;
    for k in 0..p.nb_subfr {
        let a_q12 = &p.pred_coef_q12[(k >> 1) | usize::from(!lsf_interpolation)];
        let b_q14 = &p.ltp_coef_q14[k * LTP_ORDER..][..LTP_ORDER];
        let ar_shp_q13 = &p.ar_q13[k * MAX_SHAPE_LPC_ORDER..][..MAX_SHAPE_LPC_ORDER];
        let harm_shape_fir_packed_q14 =
            (p.harm_shape_gain_q14[k] >> 2) | lshift(p.harm_shape_gain_q14[k] >> 1, 16);
        st.rewhite_flag = false;
        if p.signal_type == TYPE_VOICED {
            lag = p.pitch_l[k];
            if k & (3 - (usize::from(lsf_interpolation) << 1)) == 0 {
                // Rewhiten with the new LPC filter.
                let start_idx =
                    p.ltp_mem_length - lag as usize - p.predict_lpc_order - LTP_ORDER / 2;
                let n = p.ltp_mem_length - start_idx;
                lpc_analysis_filter(
                    &mut s_ltp[start_idx..start_idx + n],
                    &st.xq[start_idx + k * p.subfr_length..][..n],
                    &a_q12[..p.predict_lpc_order],
                );
                st.rewhite_flag = true;
                st.s_ltp_buf_idx = p.ltp_mem_length;
            }
        }
        scale_states(
            p,
            st,
            &x16[k * p.subfr_length..],
            &mut x_sc_q10,
            &s_ltp,
            &mut s_ltp_q15,
            k,
        );
        noise_shape_quantizer(
            st,
            p.signal_type,
            &x_sc_q10,
            &mut pulses[k * p.subfr_length..],
            pxq,
            &mut s_ltp_q15,
            a_q12,
            b_q14,
            ar_shp_q13,
            lag,
            harm_shape_fir_packed_q14,
            p.tilt_q14[k],
            p.lf_shp_q14[k],
            p.gains_q16[k],
            p.lambda_q10,
            offset_q10,
            p.subfr_length,
            p.shaping_lpc_order,
            p.predict_lpc_order,
        );
        pxq += p.subfr_length;
    }
    st.lag_prev = p.pitch_l[p.nb_subfr - 1];
    st.xq
        .copy_within(p.frame_length..p.frame_length + p.ltp_mem_length, 0);
    st.s_ltp_shp_q14
        .copy_within(p.frame_length..p.frame_length + p.ltp_mem_length, 0);
}

/// `silk_NSQ_noise_shape_feedback_loop_c`: the AR shaping filter's output, in Q12, its
/// state shifted along.
#[inline(always)]
pub(super) fn noise_shape_feedback_loop(
    data0: i32,
    data1: &mut [i32],
    coef: &[i16],
    order: usize,
) -> i32 {
    let mut tmp2 = data0;
    let mut tmp1 = data1[0];
    data1[0] = tmp2;
    let mut out = (order >> 1) as i32;
    out = smlawb(out, tmp2, coef[0].into());
    let mut j = 2;
    while j < order {
        tmp2 = data1[j - 1];
        data1[j - 1] = tmp1;
        out = smlawb(out, tmp1, coef[j - 1].into());
        tmp1 = data1[j];
        data1[j] = tmp2;
        out = smlawb(out, tmp2, coef[j].into());
        j += 2;
    }
    data1[order - 1] = tmp1;
    out = smlawb(out, tmp1, coef[order - 1].into());
    lshift(out, 1)
}

/// `silk_noise_shape_quantizer`: one subframe.
#[allow(clippy::too_many_arguments)]
fn noise_shape_quantizer(
    st: &mut NsqState,
    signal_type: i32,
    x_sc_q10: &[i32],
    pulses: &mut [i8],
    xq_at: usize,
    s_ltp_q15: &mut [i32],
    a_q12: &[i16],
    b_q14: &[i16],
    ar_shp_q13: &[i16],
    lag: i32,
    harm_shape_fir_packed_q14: i32,
    tilt_q14: i32,
    lf_shp_q14: i32,
    gain_q16: i32,
    lambda_q10: i32,
    offset_q10: i32,
    length: usize,
    shaping_lpc_order: usize,
    predict_lpc_order: usize,
) {
    let mut shp_lag =
        st.s_ltp_shp_buf_idx as isize - lag as isize + HARM_SHAPE_FIR_TAPS as isize / 2;
    let mut pred_lag = st.s_ltp_buf_idx as isize - lag as isize + LTP_ORDER as isize / 2;
    let gain_q10 = gain_q16 >> 6;
    let mut lpc_at = NSQ_LPC_BUF_LENGTH - 1;
    for i in 0..length {
        st.rand_seed = silk_rand(st.rand_seed);
        let lpc_pred_q10 = short_prediction(&st.s_lpc_q14, lpc_at, a_q12, predict_lpc_order);
        let ltp_pred_q13 = if signal_type == TYPE_VOICED {
            let pl = |o: isize| s_ltp_q15[(pred_lag - o) as usize];
            let mut v = 2;
            for t in 0..LTP_ORDER {
                v = smlawb(v, pl(t as isize), b_q14[t].into());
            }
            pred_lag += 1;
            v
        } else {
            0
        };

        let mut n_ar_q12 = noise_shape_feedback_loop(
            st.s_diff_shp_q14,
            &mut st.s_ar2_q14,
            ar_shp_q13,
            shaping_lpc_order,
        );
        n_ar_q12 = smlawb(n_ar_q12, st.s_lf_ar_shp_q14, tilt_q14);
        let mut n_lf_q12 = smulwb(st.s_ltp_shp_q14[st.s_ltp_shp_buf_idx - 1], lf_shp_q14);
        n_lf_q12 = smlawt(n_lf_q12, st.s_lf_ar_shp_q14, lf_shp_q14);

        let mut tmp1 = lshift(lpc_pred_q10, 2).wrapping_sub(n_ar_q12);
        tmp1 = tmp1.wrapping_sub(n_lf_q12);
        if lag > 0 {
            let sl = |o: isize| st.s_ltp_shp_q14[(shp_lag + o) as usize];
            let mut n_ltp_q13 = smulwb(add_sat32(sl(0), sl(-2)), harm_shape_fir_packed_q14);
            n_ltp_q13 = smlawt(n_ltp_q13, sl(-1), harm_shape_fir_packed_q14);
            n_ltp_q13 = lshift(n_ltp_q13, 1);
            shp_lag += 1;
            let tmp2 = ltp_pred_q13.wrapping_sub(n_ltp_q13);
            tmp1 = tmp2.wrapping_add(lshift(tmp1, 1));
            tmp1 = rshift_round(tmp1, 3);
        } else {
            tmp1 = rshift_round(tmp1, 2);
        }
        let mut r_q10 = x_sc_q10[i].wrapping_sub(tmp1);
        if st.rand_seed < 0 {
            r_q10 = r_q10.wrapping_neg();
        }
        r_q10 = r_q10.clamp(-(31 << 10), 30 << 10);

        let (q1_q10, _) = quantize_sample(r_q10, offset_q10, lambda_q10);
        pulses[i] = rshift_round(q1_q10, 10) as i8;

        let mut exc_q14 = lshift(q1_q10, 4);
        if st.rand_seed < 0 {
            exc_q14 = exc_q14.wrapping_neg();
        }
        let lpc_exc_q14 = add_lshift32(exc_q14, ltp_pred_q13, 1);
        let xq_q14 = lpc_exc_q14.wrapping_add(lshift(lpc_pred_q10, 4));
        st.xq[xq_at + i] = sat16(rshift_round(smulww(xq_q14, gain_q10), 8));

        lpc_at += 1;
        st.s_lpc_q14[lpc_at] = xq_q14;
        st.s_diff_shp_q14 = sub_lshift32(xq_q14, x_sc_q10[i], 4);
        let s_lf_ar_shp_q14 = st.s_diff_shp_q14.wrapping_sub(lshift(n_ar_q12, 2));
        st.s_lf_ar_shp_q14 = s_lf_ar_shp_q14;
        st.s_ltp_shp_q14[st.s_ltp_shp_buf_idx] = s_lf_ar_shp_q14.wrapping_sub(lshift(n_lf_q12, 2));
        s_ltp_q15[st.s_ltp_buf_idx] = lshift(lpc_exc_q14, 1);
        st.s_ltp_shp_buf_idx += 1;
        st.s_ltp_buf_idx += 1;
        st.rand_seed = st.rand_seed.wrapping_add(i32::from(pulses[i]));
    }
    st.s_lpc_q14
        .copy_within(length..length + NSQ_LPC_BUF_LENGTH, 0);
}

/// The two quantisation candidates around `r_q10` and the rate-distortion choice between them
/// (shared by both quantisers): returns the chosen level and, for the delayed decision, both
/// candidates with their costs `(q1, rd1, q2, rd2)`.
#[inline(always)]
pub(super) fn quantize_sample(r_q10: i32, offset_q10: i32, lambda_q10: i32) -> (i32, [i32; 4]) {
    let mut q1_q10 = r_q10 - offset_q10;
    let mut q1_q0 = q1_q10 >> 10;
    if lambda_q10 > 2048 {
        // For aggressive rate-distortion optimisation the bias exceeds a pulse.
        let rdo_offset = lambda_q10 / 2 - 512;
        q1_q0 = if q1_q10 > rdo_offset {
            (q1_q10 - rdo_offset) >> 10
        } else if q1_q10 < -rdo_offset {
            (q1_q10 + rdo_offset) >> 10
        } else if q1_q10 < 0 {
            -1
        } else {
            0
        };
    }
    let (q2_q10, mut rd1_q20, mut rd2_q20);
    if q1_q0 > 0 {
        q1_q10 = lshift(q1_q0, 10) - QUANT_LEVEL_ADJUST_Q10 + offset_q10;
        q2_q10 = q1_q10 + 1024;
        rd1_q20 = smulbb(q1_q10, lambda_q10);
        rd2_q20 = smulbb(q2_q10, lambda_q10);
    } else if q1_q0 == 0 {
        q1_q10 = offset_q10;
        q2_q10 = q1_q10 + 1024 - QUANT_LEVEL_ADJUST_Q10;
        rd1_q20 = smulbb(q1_q10, lambda_q10);
        rd2_q20 = smulbb(q2_q10, lambda_q10);
    } else if q1_q0 == -1 {
        q2_q10 = offset_q10;
        q1_q10 = q2_q10 - (1024 - QUANT_LEVEL_ADJUST_Q10);
        rd1_q20 = smulbb(-q1_q10, lambda_q10);
        rd2_q20 = smulbb(q2_q10, lambda_q10);
    } else {
        q1_q10 = lshift(q1_q0, 10) + QUANT_LEVEL_ADJUST_Q10 + offset_q10;
        q2_q10 = q1_q10 + 1024;
        rd1_q20 = smulbb(-q1_q10, lambda_q10);
        rd2_q20 = smulbb(-q2_q10, lambda_q10);
    }
    let rr_q10 = r_q10 - q1_q10;
    rd1_q20 = smlabb(rd1_q20, rr_q10, rr_q10);
    let rr_q10 = r_q10 - q2_q10;
    rd2_q20 = smlabb(rd2_q20, rr_q10, rr_q10);
    let chosen = if rd2_q20 < rd1_q20 { q2_q10 } else { q1_q10 };
    (chosen, [q1_q10, rd1_q20, q2_q10, rd2_q20])
}

/// `silk_nsq_scale_states`: the input scaled by the subframe's inverse gain, and the states
/// rescaled when the gain changes.
fn scale_states(
    p: &NsqParams<'_>,
    st: &mut NsqState,
    x16: &[i16],
    x_sc_q10: &mut [i32],
    s_ltp: &[i16],
    s_ltp_q15: &mut [i32],
    subfr: usize,
) {
    let lag = p.pitch_l[subfr] as usize;
    let mut inv_gain_q31 = inverse32_varq(p.gains_q16[subfr].max(1), 47);
    let inv_gain_q26 = rshift_round(inv_gain_q31, 5);
    for i in 0..p.subfr_length {
        x_sc_q10[i] = smulww(x16[i].into(), inv_gain_q26);
    }
    if st.rewhite_flag {
        if subfr == 0 {
            // LTP downscaling.
            inv_gain_q31 = lshift(smulwb(inv_gain_q31, p.ltp_scale_q14), 2);
        }
        for i in st.s_ltp_buf_idx - lag - LTP_ORDER / 2..st.s_ltp_buf_idx {
            s_ltp_q15[i] = smulwb(inv_gain_q31, s_ltp[i].into());
        }
    }
    if p.gains_q16[subfr] != st.prev_gain_q16 {
        let gain_adj_q16 = div32_varq(st.prev_gain_q16, p.gains_q16[subfr], 16);
        for i in st.s_ltp_shp_buf_idx - p.ltp_mem_length..st.s_ltp_shp_buf_idx {
            st.s_ltp_shp_q14[i] = smulww(gain_adj_q16, st.s_ltp_shp_q14[i]);
        }
        if p.signal_type == TYPE_VOICED && !st.rewhite_flag {
            for i in st.s_ltp_buf_idx - lag - LTP_ORDER / 2..st.s_ltp_buf_idx {
                s_ltp_q15[i] = smulww(gain_adj_q16, s_ltp_q15[i]);
            }
        }
        st.s_lf_ar_shp_q14 = smulww(gain_adj_q16, st.s_lf_ar_shp_q14);
        st.s_diff_shp_q14 = smulww(gain_adj_q16, st.s_diff_shp_q14);
        for v in st.s_lpc_q14[..NSQ_LPC_BUF_LENGTH].iter_mut() {
            *v = smulww(gain_adj_q16, *v);
        }
        for v in st.s_ar2_q14.iter_mut() {
            *v = smulww(gain_adj_q16, *v);
        }
        st.prev_gain_q16 = p.gains_q16[subfr];
    }
}
