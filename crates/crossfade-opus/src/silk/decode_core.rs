//! Reconstruction (libopus `silk/decode_core.c`, `LPC_analysis_filter.c`; RFC 6716 4.2.7.9):
//! the excitation from the pulses and the seed, then per subframe the long-term (pitch)
//! prediction, re-whitening the past output where the filters change, and the short-term
//! (LPC) synthesis with gain scaling.

use super::macros::{
    div32_varq, inverse32_varq, lshift, lshift_sat32, rshift_round, sat16, silk_rand, smlabb,
    smlawb, smulbb, smulwb, smulww,
};
use super::state::{
    ChannelState, DecoderControl, LTP_ORDER, MAX_FRAME_LENGTH, MAX_LPC_ORDER, MAX_NB_SUBFR,
    MAX_SUB_FRAME_LENGTH, TYPE_VOICED,
};
use super::tables::QUANTIZATION_OFFSETS_Q10;

const QUANT_LEVEL_ADJUST_Q10: i32 = 80;
/// `LTP_MEM_LENGTH_MS * MAX_FS_KHZ`.
const MAX_LTP_MEM_LENGTH: usize = 320;

/// The pitch lag a buffer of `ltp_mem_length` can serve with an LPC of `lpc_order`: valid
/// streams stay within it (`decode_pitch` limits lags to 18 ms of the 20 ms memory); the
/// clamp keeps a lag from elsewhere (concealment's) inside the buffers.
pub(super) fn safe_lag(lag: i32, ltp_mem_length: usize, lpc_order: usize) -> usize {
    let max = ltp_mem_length - lpc_order - LTP_ORDER / 2 - 1;
    (lag.max(0) as usize).clamp(LTP_ORDER / 2 + 1, max)
}

/// `silk_decode_core`: `xq` gets the frame's `frame_length` output samples.
pub(super) fn decode_core(
    st: &mut ChannelState,
    ctrl: &mut DecoderControl,
    xq: &mut [i16],
    pulses: &[i16; MAX_FRAME_LENGTH],
) {
    let frame_length = st.frame_length;
    let subfr_length = st.subfr_length;
    let ltp_mem_length = st.ltp_mem_length;
    let order = st.lpc_order;
    let mut s_ltp = [0i16; MAX_LTP_MEM_LENGTH];
    let mut s_ltp_q15 = [0i32; MAX_LTP_MEM_LENGTH + MAX_FRAME_LENGTH];
    let mut res_q14 = [0i32; MAX_SUB_FRAME_LENGTH];
    let mut s_lpc_q14 = [0i32; MAX_SUB_FRAME_LENGTH + MAX_LPC_ORDER];

    let offset_q10 = i32::from(
        QUANTIZATION_OFFSETS_Q10[(st.indices.signal_type >> 1) as usize]
            [st.indices.quant_offset_type as usize],
    );
    let nlsf_interpolation = st.indices.nlsf_interp_coef_q2 < 4;

    // The excitation: pulses, moved towards zero by the quantiser's level adjustment, offset,
    // and given pseudo-random signs.
    let mut rand_seed = i32::from(st.indices.seed);
    for i in 0..frame_length {
        rand_seed = silk_rand(rand_seed);
        let mut e = i32::from(pulses[i]) << 14;
        if e > 0 {
            e -= QUANT_LEVEL_ADJUST_Q10 << 4;
        } else if e < 0 {
            e += QUANT_LEVEL_ADJUST_Q10 << 4;
        }
        e += offset_q10 << 4;
        if rand_seed < 0 {
            e = -e;
        }
        st.exc_q14[i] = e;
        rand_seed = rand_seed.wrapping_add(i32::from(pulses[i]));
    }

    s_lpc_q14[..MAX_LPC_ORDER].copy_from_slice(&st.slpc_q14_buf);
    let mut s_ltp_buf_idx = ltp_mem_length;
    let mut lag = 0usize;
    for k in 0..st.nb_subfr.min(MAX_NB_SUBFR) {
        let a_q12: [i16; MAX_LPC_ORDER] = ctrl.pred_coef_q12[k >> 1];
        let mut signal_type = i32::from(st.indices.signal_type);
        let gain_q10 = ctrl.gains_q16[k] >> 6;
        let mut inv_gain_q31 = inverse32_varq(ctrl.gains_q16[k], 47);

        // A gain change rescales the LPC state.
        let gain_adj_q16 = if ctrl.gains_q16[k] != st.prev_gain_q16 {
            let adj = div32_varq(st.prev_gain_q16, ctrl.gains_q16[k], 16);
            for s in &mut s_lpc_q14[..MAX_LPC_ORDER] {
                *s = smulww(adj, *s);
            }
            adj
        } else {
            1 << 16
        };
        st.prev_gain_q16 = ctrl.gains_q16[k];

        // After concealing a voiced frame, an unvoiced one keeps a little pitch for its first
        // half.
        if st.loss_cnt != 0
            && st.prev_signal_type == TYPE_VOICED
            && signal_type != TYPE_VOICED
            && k < MAX_NB_SUBFR / 2
        {
            let b = &mut ctrl.ltp_coef_q14[k * LTP_ORDER..][..LTP_ORDER];
            b.fill(0);
            b[LTP_ORDER / 2] = 4096; // SILK_FIX_CONST(0.25, 14)
            signal_type = TYPE_VOICED;
            ctrl.pitch_l[k] = st.lag_prev;
        }

        if signal_type == TYPE_VOICED {
            lag = safe_lag(ctrl.pitch_l[k], ltp_mem_length, order);
            if k == 0 || (k == 2 && nlsf_interpolation) {
                // Re-whiten the past output with this subframe's filter.
                let start_idx = ltp_mem_length - lag - order - LTP_ORDER / 2;
                if k == 2 {
                    st.out_buf[ltp_mem_length..ltp_mem_length + 2 * subfr_length]
                        .copy_from_slice(&xq[..2 * subfr_length]);
                }
                lpc_analysis_filter(
                    &mut s_ltp[start_idx..ltp_mem_length],
                    &st.out_buf[start_idx + k * subfr_length..ltp_mem_length + k * subfr_length],
                    &a_q12[..order],
                );
                if k == 0 {
                    // LTP scaling lowers the excitation's dependence on a possibly lost past.
                    inv_gain_q31 = lshift(smulwb(inv_gain_q31, ctrl.ltp_scale_q14), 2);
                }
                for i in 0..lag + LTP_ORDER / 2 {
                    s_ltp_q15[s_ltp_buf_idx - i - 1] =
                        smulwb(inv_gain_q31, s_ltp[ltp_mem_length - i - 1].into());
                }
            } else if gain_adj_q16 != 1 << 16 {
                for i in 0..lag + LTP_ORDER / 2 {
                    let s = &mut s_ltp_q15[s_ltp_buf_idx - i - 1];
                    *s = smulww(gain_adj_q16, *s);
                }
            }
        }

        let exc = &st.exc_q14[k * subfr_length..][..subfr_length];
        let res: &[i32] = if signal_type == TYPE_VOICED {
            let b = &ctrl.ltp_coef_q14[k * LTP_ORDER..][..LTP_ORDER];
            for i in 0..subfr_length {
                let p = s_ltp_buf_idx - lag + LTP_ORDER / 2;
                let mut pred_q13 = 2;
                for (j, &bj) in b.iter().enumerate() {
                    pred_q13 = smlawb(pred_q13, s_ltp_q15[p - j], bj.into());
                }
                res_q14[i] = exc[i].wrapping_add(lshift(pred_q13, 1));
                s_ltp_q15[s_ltp_buf_idx] = lshift(res_q14[i], 1);
                s_ltp_buf_idx += 1;
            }
            &res_q14[..subfr_length]
        } else {
            exc
        };

        let out = &mut xq[k * subfr_length..][..subfr_length];
        for i in 0..subfr_length {
            let pred_q10 = lpc_predict(&s_lpc_q14, MAX_LPC_ORDER + i, &a_q12, order);
            let s = res[i].saturating_add(lshift_sat32(pred_q10, 4));
            s_lpc_q14[MAX_LPC_ORDER + i] = s;
            out[i] = sat16(rshift_round(smulww(s, gain_q10), 8));
        }
        s_lpc_q14.copy_within(subfr_length..subfr_length + MAX_LPC_ORDER, 0);
    }
    st.slpc_q14_buf.copy_from_slice(&s_lpc_q14[..MAX_LPC_ORDER]);
}

/// The LPC prediction of `s[at]` in Q10 from the `order` samples before it (the unrolled sums
/// of `decode_core`, `PLC` and `CNG`), rounding bias `order / 2` included.
#[inline(always)]
pub(super) fn lpc_predict(s: &[i32], at: usize, a_q12: &[i16; MAX_LPC_ORDER], order: usize) -> i32 {
    let mut pred = (order >> 1) as i32;
    for j in 0..order {
        pred = smlawb(pred, s[at - j - 1], a_q12[j].into());
    }
    pred
}

/// `silk_LPC_analysis_filter`: `out[i]` is `input[i]` minus its prediction from the `order`
/// samples before it; the first `order` outputs are 0. The sums may wrap, so that two wraps
/// cancel, as in the C.
pub(super) fn lpc_analysis_filter(out: &mut [i16], input: &[i16], b: &[i16]) {
    let d = b.len();
    let len = out.len().min(input.len());
    for ix in d..len {
        let mut acc = smulbb(input[ix - 1].into(), b[0].into());
        for j in 1..d {
            acc = smlabb(acc, input[ix - 1 - j].into(), b[j].into());
        }
        let v = (i32::from(input[ix]) << 12).wrapping_sub(acc);
        out[ix] = sat16(rshift_round(v, 12));
    }
    out[..d.min(len)].fill(0);
}
