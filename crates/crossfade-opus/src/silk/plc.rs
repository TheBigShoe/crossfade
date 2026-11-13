//! Packet loss concealment (libopus `silk/PLC.c`, the plain path without the deep PLC): good
//! frames update a pitch and LPC model; a lost frame is extrapolated from it, pitch-synchronous
//! for voiced speech and noise-excited otherwise, attenuating with every further loss; the
//! first good frame after a loss is faded in when it is louder than the concealment.

use super::decode_core::{lpc_analysis_filter, lpc_predict, safe_lag};
use super::macros::{
    clz32, div32, inverse32_varq, lshift, lshift_sat32, rshift_round, sat16, silk_rand, smlawb,
    smulbb, smulwb, smulww, sqrt_approx, sum_sqr_shift,
};
use super::nlsf::{bwexpander, lpc_inverse_pred_gain};
use super::state::{
    ChannelState, DecoderControl, LTP_ORDER, MAX_FRAME_LENGTH, MAX_LPC_ORDER, MAX_NB_SUBFR,
    MAX_SUB_FRAME_LENGTH, TYPE_VOICED,
};

const NB_ATT: usize = 2;
const HARM_ATT_Q15: [i32; NB_ATT] = [32440, 31130];
const PLC_RAND_ATTENUATE_V_Q15: [i32; NB_ATT] = [31130, 26214];
const PLC_RAND_ATTENUATE_UV_Q15: [i32; NB_ATT] = [32440, 29491];
const BWE_COEF_Q16: i32 = 64881; // SILK_FIX_CONST(0.99, 16)
const V_PITCH_GAIN_START_MIN_Q14: i32 = 11469;
const V_PITCH_GAIN_START_MAX_Q14: i32 = 15565;
const MAX_PITCH_LAG_MS: i32 = 18;
const RAND_BUF_SIZE: usize = 128;
const RAND_BUF_MASK: i32 = RAND_BUF_SIZE as i32 - 1;
const LOG2_INV_LPC_GAIN_HIGH_THRES: u32 = 3;
const LOG2_INV_LPC_GAIN_LOW_THRES: u32 = 8;
const PITCH_DRIFT_FAC_Q16: i32 = 655;
const MAX_LTP_MEM_LENGTH: usize = 320;

/// `silk_PLC_Reset`.
pub(super) fn plc_reset(st: &mut ChannelState) {
    st.plc.pitch_l_q8 = lshift(st.frame_length as i32, 7);
    st.plc.prev_gain_q16 = [1 << 16, 1 << 16];
    st.plc.subfr_length = 20;
    st.plc.nb_subfr = 2;
}

/// `silk_PLC`: conceal into `frame` when `lost`, otherwise learn from the frame just decoded.
pub(super) fn plc(st: &mut ChannelState, ctrl: &mut DecoderControl, frame: &mut [i16], lost: bool) {
    if st.fs_khz != st.plc.fs_khz {
        plc_reset(st);
        st.plc.fs_khz = st.fs_khz;
    }
    if lost {
        conceal(st, ctrl, frame);
        st.loss_cnt += 1;
    } else {
        update(st, ctrl);
    }
}

/// `silk_PLC_update`.
fn update(st: &mut ChannelState, ctrl: &DecoderControl) {
    let nb_subfr = st.nb_subfr.min(MAX_NB_SUBFR);
    let plc = &mut st.plc;
    st.prev_signal_type = i32::from(st.indices.signal_type);
    let mut ltp_gain_q14 = 0i32;
    if i32::from(st.indices.signal_type) == TYPE_VOICED {
        // The strongest LTP filter within the last pitch period.
        let mut j = 0;
        while (j * st.subfr_length) < ctrl.pitch_l[nb_subfr - 1].max(0) as usize {
            if j == nb_subfr {
                break;
            }
            let row = &ctrl.ltp_coef_q14[(nb_subfr - 1 - j) * LTP_ORDER..][..LTP_ORDER];
            let temp: i32 = row.iter().map(|&c| i32::from(c)).sum();
            if temp > ltp_gain_q14 {
                ltp_gain_q14 = temp;
                plc.ltp_coef_q14.copy_from_slice(row);
                plc.pitch_l_q8 = lshift(ctrl.pitch_l[nb_subfr - 1 - j], 8);
            }
            j += 1;
        }
        plc.ltp_coef_q14 = [0; LTP_ORDER];
        plc.ltp_coef_q14[LTP_ORDER / 2] = ltp_gain_q14 as i16;
        // Keep the concealment's pitch gain between 0.7 and 0.95.
        if ltp_gain_q14 < V_PITCH_GAIN_START_MIN_Q14 {
            let scale_q10 = div32(V_PITCH_GAIN_START_MIN_Q14 << 10, ltp_gain_q14.max(1));
            for c in &mut plc.ltp_coef_q14 {
                *c = (smulbb((*c).into(), scale_q10) >> 10) as i16;
            }
        } else if ltp_gain_q14 > V_PITCH_GAIN_START_MAX_Q14 {
            let scale_q14 = div32(V_PITCH_GAIN_START_MAX_Q14 << 14, ltp_gain_q14.max(1));
            for c in &mut plc.ltp_coef_q14 {
                *c = (smulbb((*c).into(), scale_q14) >> 14) as i16;
            }
        }
    } else {
        plc.pitch_l_q8 = lshift(smulbb(st.fs_khz, 18), 8);
        plc.ltp_coef_q14 = [0; LTP_ORDER];
    }
    plc.prev_lpc_q12[..st.lpc_order].copy_from_slice(&ctrl.pred_coef_q12[1][..st.lpc_order]);
    plc.prev_ltp_scale_q14 = ctrl.ltp_scale_q14 as i16;
    plc.prev_gain_q16 = [ctrl.gains_q16[nb_subfr - 2], ctrl.gains_q16[nb_subfr - 1]];
    plc.subfr_length = st.subfr_length;
    plc.nb_subfr = nb_subfr;
}

/// `silk_PLC_energy`: the energies of the last two subframes' excitation.
fn energy(st: &ChannelState, prev_gain_q10: [i32; 2]) -> (i32, i32, i32, i32) {
    let subfr = st.subfr_length;
    let mut buf = [0i16; 2 * MAX_SUB_FRAME_LENGTH];
    for k in 0..2 {
        let base = (k + st.nb_subfr - 2) * subfr;
        for i in 0..subfr {
            buf[k * subfr + i] = sat16(smulww(st.exc_q14[base + i], prev_gain_q10[k]) >> 8);
        }
    }
    let (e1, s1) = sum_sqr_shift(&buf[..subfr]);
    let (e2, s2) = sum_sqr_shift(&buf[subfr..2 * subfr]);
    (e1, s1, e2, s2)
}

/// `silk_PLC_conceal`.
fn conceal(st: &mut ChannelState, ctrl: &mut DecoderControl, frame: &mut [i16]) {
    let order = st.lpc_order;
    let ltp_mem_length = st.ltp_mem_length;
    let frame_length = st.frame_length;
    let prev_gain_q10 = [st.plc.prev_gain_q16[0] >> 6, st.plc.prev_gain_q16[1] >> 6];
    if st.first_frame_after_reset {
        st.plc.prev_lpc_q12 = [0; MAX_LPC_ORDER];
    }

    // Take the noise from the quieter of the last two subframes' excitation.
    let (e1, s1, e2, s2) = energy(st, prev_gain_q10);
    let plc_len = st.plc.nb_subfr * st.plc.subfr_length;
    let rand_base =
        if e1.checked_shr(s2 as u32).unwrap_or(0) < e2.checked_shr(s1 as u32).unwrap_or(0) {
            (plc_len - st.plc.subfr_length).saturating_sub(RAND_BUF_SIZE)
        } else {
            plc_len.saturating_sub(RAND_BUF_SIZE)
        }
        .min(MAX_FRAME_LENGTH - RAND_BUF_SIZE);

    let att = (st.loss_cnt as usize).min(NB_ATT - 1);
    let harm_gain_q15 = HARM_ATT_Q15[att];
    let mut rand_gain_q15 = if st.prev_signal_type == TYPE_VOICED {
        PLC_RAND_ATTENUATE_V_Q15[att]
    } else {
        PLC_RAND_ATTENUATE_UV_Q15[att]
    };

    bwexpander(&mut st.plc.prev_lpc_q12[..order], BWE_COEF_Q16);
    let a_q12 = st.plc.prev_lpc_q12;

    let mut rand_scale_q14 = st.plc.rand_scale_q14;
    if st.loss_cnt == 0 {
        rand_scale_q14 = 1 << 14;
        if st.prev_signal_type == TYPE_VOICED {
            let mut s = i32::from(rand_scale_q14);
            for &b in &st.plc.ltp_coef_q14 {
                s -= i32::from(b);
            }
            let s = s.max(3277) as i16; // 0.2
            rand_scale_q14 = (smulbb(s.into(), st.plc.prev_ltp_scale_q14.into()) >> 14) as i16;
        } else {
            // Less noise for a strongly resonant filter.
            let inv_gain_q30 = lpc_inverse_pred_gain(&st.plc.prev_lpc_q12[..order]);
            let down = inv_gain_q30.clamp(
                (1 << 30) >> LOG2_INV_LPC_GAIN_LOW_THRES,
                (1 << 30) >> LOG2_INV_LPC_GAIN_HIGH_THRES,
            ) << LOG2_INV_LPC_GAIN_HIGH_THRES;
            rand_gain_q15 = smulwb(down, rand_gain_q15) >> 14;
        }
    }

    let mut rand_seed = st.plc.rand_seed;
    let mut lag = safe_lag(rshift_round(st.plc.pitch_l_q8, 8), ltp_mem_length, order);
    let mut s_ltp_buf_idx = ltp_mem_length;

    // Re-whiten the last output into the LTP buffer, scaled by the last gain's inverse.
    let mut s_ltp = [0i16; MAX_LTP_MEM_LENGTH];
    let mut s_ltp_q14 = [0i32; MAX_LTP_MEM_LENGTH + MAX_FRAME_LENGTH];
    let idx = ltp_mem_length - lag - order - LTP_ORDER / 2;
    lpc_analysis_filter(
        &mut s_ltp[idx..ltp_mem_length],
        &st.out_buf[idx..ltp_mem_length],
        &a_q12[..order],
    );
    let inv_gain_q30 = inverse32_varq(st.plc.prev_gain_q16[1], 46).min(i32::MAX >> 1);
    for i in idx + order..ltp_mem_length {
        s_ltp_q14[i] = smulwb(inv_gain_q30, s_ltp[i].into());
    }

    let b_q14 = &mut st.plc.ltp_coef_q14;
    for _ in 0..st.nb_subfr.min(MAX_NB_SUBFR) {
        let first = s_ltp_buf_idx - lag + LTP_ORDER / 2;
        for p in first..first + st.subfr_length {
            let mut pred_q12 = 2;
            for (j, &b) in b_q14.iter().enumerate() {
                pred_q12 = smlawb(pred_q12, s_ltp_q14[p - j], b.into());
            }
            rand_seed = silk_rand(rand_seed);
            let r = ((rand_seed >> 25) & RAND_BUF_MASK) as usize;
            let noise = st.exc_q14[rand_base + r];
            s_ltp_q14[s_ltp_buf_idx] = lshift(smlawb(pred_q12, noise, rand_scale_q14.into()), 2);
            s_ltp_buf_idx += 1;
        }
        for b in b_q14.iter_mut() {
            *b = (smulbb(harm_gain_q15, (*b).into()) >> 15) as i16;
        }
        rand_scale_q14 = (smulbb(rand_scale_q14.into(), rand_gain_q15) >> 15) as i16;
        // Let the pitch drift slowly upwards.
        let p_q8 = smlawb(st.plc.pitch_l_q8, st.plc.pitch_l_q8, PITCH_DRIFT_FAC_Q16);
        st.plc.pitch_l_q8 = p_q8.min(lshift(smulbb(MAX_PITCH_LAG_MS, st.fs_khz), 8));
        lag = safe_lag(rshift_round(st.plc.pitch_l_q8, 8), ltp_mem_length, order);
    }

    // LPC synthesis over the excitation just made, continuing the decoder's LPC state.
    let base = ltp_mem_length - MAX_LPC_ORDER;
    s_ltp_q14[base..ltp_mem_length].copy_from_slice(&st.slpc_q14_buf);
    for i in 0..frame_length {
        let at = ltp_mem_length + i;
        let pred_q10 = lpc_predict(&s_ltp_q14, at, &a_q12, order);
        s_ltp_q14[at] = s_ltp_q14[at].saturating_add(lshift_sat32(pred_q10, 4));
        frame[i] = sat16(rshift_round(smulww(s_ltp_q14[at], prev_gain_q10[1]), 8));
    }
    st.slpc_q14_buf
        .copy_from_slice(&s_ltp_q14[base + frame_length..base + frame_length + MAX_LPC_ORDER]);

    st.plc.rand_seed = rand_seed;
    st.plc.rand_scale_q14 = rand_scale_q14;
    ctrl.pitch_l = [lag as i32; MAX_NB_SUBFR];
}

/// `silk_PLC_glue_frames`: after a loss, fade the first good frame in from the concealment's
/// energy when it is louder.
pub(super) fn glue_frames(st: &mut ChannelState, frame: &mut [i16]) {
    let plc = &mut st.plc;
    if st.loss_cnt != 0 {
        (plc.conc_energy, plc.conc_energy_shift) = sum_sqr_shift(frame);
        plc.last_frame_lost = true;
        return;
    }
    if plc.last_frame_lost {
        let (mut energy, shift) = sum_sqr_shift(frame);
        if shift > plc.conc_energy_shift {
            plc.conc_energy >>= shift - plc.conc_energy_shift;
        } else if shift < plc.conc_energy_shift {
            energy >>= plc.conc_energy_shift - shift;
        }
        if energy > plc.conc_energy {
            let lz = (clz32(plc.conc_energy) - 1).max(0);
            plc.conc_energy = lshift(plc.conc_energy, lz as u32);
            energy >>= (24 - lz).max(0);
            let frac_q24 = div32(plc.conc_energy, energy.max(1));
            let mut gain_q16 = lshift(sqrt_approx(frac_q24), 4);
            let len = frame.len().max(1) as i32;
            let slope_q16 = lshift(div32((1 << 16) - gain_q16, len), 2);
            for s in frame.iter_mut() {
                *s = smulwb(gain_q16, (*s).into()) as i16;
                gain_q16 += slope_q16;
                if gain_q16 > 1 << 16 {
                    break;
                }
            }
        }
    }
    plc.last_frame_lost = false;
}
