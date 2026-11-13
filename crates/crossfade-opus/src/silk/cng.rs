//! Comfort noise generation (libopus `silk/CNG.c`): during inactive frames the decoder tracks a
//! smoothed spectrum and gain and keeps recent excitation; while concealing it adds noise
//! shaped by them, so a long loss fades into background noise rather than silence.

use super::decode_core::lpc_predict;
use super::macros::{
    add_sat16, lshift, lshift_sat32, rshift_round, sat16, silk_rand, smultt, smulwb, smulww,
    sqrt_approx,
};
use super::nlsf::nlsf2a;
use super::state::{
    ChannelState, DecoderControl, MAX_FRAME_LENGTH, MAX_LPC_ORDER, MAX_NB_SUBFR,
    TYPE_NO_VOICE_ACTIVITY,
};

const CNG_BUF_MASK_MAX: i32 = 255;
const CNG_GAIN_SMTH_Q16: i32 = 4634;
const CNG_GAIN_SMTH_THRESHOLD_Q16: i32 = 46396;
const CNG_NLSF_SMTH_Q16: i32 = 16348;

/// `silk_CNG_Reset`.
pub(super) fn cng_reset(st: &mut ChannelState) {
    let step = i32::from(i16::MAX) / (st.lpc_order as i32 + 1);
    let mut acc = 0;
    for v in &mut st.cng.smth_nlsf_q15[..st.lpc_order] {
        acc += step;
        *v = acc as i16;
    }
    st.cng.smth_gain_q16 = 0;
    st.cng.rand_seed = 3_176_576;
}

/// `silk_CNG`.
pub(super) fn cng(st: &mut ChannelState, ctrl: &DecoderControl, frame: &mut [i16]) {
    if st.fs_khz != st.cng.fs_khz {
        cng_reset(st);
        st.cng.fs_khz = st.fs_khz;
    }
    let order = st.lpc_order;
    let nb_subfr = st.nb_subfr.min(MAX_NB_SUBFR);
    let subfr = st.subfr_length;
    if st.loss_cnt == 0 && st.prev_signal_type == TYPE_NO_VOICE_ACTIVITY {
        // Smooth the spectrum towards the last frame's.
        for i in 0..order {
            let d = i32::from(st.prev_nlsf_q15[i]) - i32::from(st.cng.smth_nlsf_q15[i]);
            st.cng.smth_nlsf_q15[i] =
                (i32::from(st.cng.smth_nlsf_q15[i]) + smulwb(d, CNG_NLSF_SMTH_Q16)) as i16;
        }
        // Keep the excitation of the loudest subframe.
        let mut max_gain = 0;
        let mut loudest = 0;
        for i in 0..nb_subfr {
            if ctrl.gains_q16[i] > max_gain {
                max_gain = ctrl.gains_q16[i];
                loudest = i;
            }
        }
        st.cng
            .exc_buf_q14
            .copy_within(0..(nb_subfr - 1) * subfr, subfr);
        st.cng.exc_buf_q14[..subfr].copy_from_slice(&st.exc_q14[loudest * subfr..][..subfr]);
        for i in 0..nb_subfr {
            let g = ctrl.gains_q16[i];
            st.cng.smth_gain_q16 = st.cng.smth_gain_q16.wrapping_add(smulwb(
                g.wrapping_sub(st.cng.smth_gain_q16),
                CNG_GAIN_SMTH_Q16,
            ));
            // Follow a fall of more than 3 dB at once.
            if smulww(st.cng.smth_gain_q16, CNG_GAIN_SMTH_THRESHOLD_Q16) > g {
                st.cng.smth_gain_q16 = g;
            }
        }
    }

    if st.loss_cnt != 0 {
        let length = frame.len();
        let mut sig_q14 = [0i32; MAX_FRAME_LENGTH + MAX_LPC_ORDER];
        // The noise fills what the concealment's own noise does not.
        let mut gain_q16 = smulww(st.plc.rand_scale_q14.into(), st.plc.prev_gain_q16[1]);
        if gain_q16 >= 1 << 21 || st.cng.smth_gain_q16 > 1 << 23 {
            gain_q16 = smultt(gain_q16, gain_q16);
            gain_q16 = smultt(st.cng.smth_gain_q16, st.cng.smth_gain_q16)
                .wrapping_sub(lshift(gain_q16, 5));
            gain_q16 = lshift(sqrt_approx(gain_q16), 16);
        } else {
            gain_q16 = smulww(gain_q16, gain_q16);
            gain_q16 = smulww(st.cng.smth_gain_q16, st.cng.smth_gain_q16)
                .wrapping_sub(lshift(gain_q16, 5));
            gain_q16 = lshift(sqrt_approx(gain_q16), 8);
        }
        let gain_q10 = gain_q16 >> 6;

        // silk_CNG_exc: random picks from the kept excitation.
        let mut mask = CNG_BUF_MASK_MAX;
        while mask > length as i32 {
            mask >>= 1;
        }
        let mut seed = st.cng.rand_seed;
        for s in &mut sig_q14[MAX_LPC_ORDER..MAX_LPC_ORDER + length] {
            seed = silk_rand(seed);
            *s = st.cng.exc_buf_q14[((seed >> 24) & mask) as usize];
        }
        st.cng.rand_seed = seed;

        let mut a_q12 = [0i16; MAX_LPC_ORDER];
        nlsf2a(&mut a_q12[..order], &st.cng.smth_nlsf_q15[..order]);
        sig_q14[..MAX_LPC_ORDER].copy_from_slice(&st.cng.synth_state);
        for i in 0..length {
            let at = MAX_LPC_ORDER + i;
            let pred_q10 = lpc_predict(&sig_q14, at, &a_q12, order);
            sig_q14[at] = sig_q14[at].saturating_add(lshift_sat32(pred_q10, 4));
            frame[i] = add_sat16(
                frame[i],
                sat16(rshift_round(smulww(sig_q14[at], gain_q10), 8)).into(),
            );
        }
        st.cng
            .synth_state
            .copy_from_slice(&sig_q14[length..length + MAX_LPC_ORDER]);
    } else {
        st.cng.synth_state[..order].fill(0);
    }
}
