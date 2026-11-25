//! The delayed-decision noise-shaping quantiser (libopus `silk/NSQ_del_dec.c`): several
//! candidate quantisations run side by side and the best is committed `DECISION_DELAY`
//! samples late, with warped noise shaping.

use super::fix::{
    add_sat32, div32_varq, inverse32_varq, lshift, rshift_round, sat16, silk_rand, smlawb, smlawt,
    smulwb, smulww, sub_lshift32, sub_sat32,
};
use super::nsq::{HARM_SHAPE_FIR_TAPS, NsqParams, quantize_sample, short_prediction};
use super::sigproc::MAX_SHAPE_LPC_ORDER;
use super::state::{NSQ_LPC_BUF_LENGTH, NsqState};
use crate::silk::decode_core::lpc_analysis_filter;
use crate::silk::state::{LTP_ORDER, MAX_FRAME_LENGTH, MAX_SUB_FRAME_LENGTH, TYPE_VOICED};
use crate::silk::tables::QUANTIZATION_OFFSETS_Q10;

const DECISION_DELAY: usize = 40;
const MAX_DEL_DEC_STATES: usize = 4;

/// `NSQ_del_dec_struct`: one candidate's state.
#[derive(Debug, Clone, Copy)]
struct DelDec {
    s_lpc_q14: [i32; MAX_SUB_FRAME_LENGTH + NSQ_LPC_BUF_LENGTH],
    rand_state: [i32; DECISION_DELAY],
    q_q10: [i32; DECISION_DELAY],
    xq_q14: [i32; DECISION_DELAY],
    pred_q15: [i32; DECISION_DELAY],
    shape_q14: [i32; DECISION_DELAY],
    s_ar2_q14: [i32; MAX_SHAPE_LPC_ORDER],
    lf_ar_q14: i32,
    diff_q14: i32,
    seed: i32,
    seed_init: i32,
    rd_q10: i32,
}

impl Default for DelDec {
    fn default() -> Self {
        Self {
            s_lpc_q14: [0; MAX_SUB_FRAME_LENGTH + NSQ_LPC_BUF_LENGTH],
            rand_state: [0; DECISION_DELAY],
            q_q10: [0; DECISION_DELAY],
            xq_q14: [0; DECISION_DELAY],
            pred_q15: [0; DECISION_DELAY],
            shape_q14: [0; DECISION_DELAY],
            s_ar2_q14: [0; MAX_SHAPE_LPC_ORDER],
            lf_ar_q14: 0,
            diff_q14: 0,
            seed: 0,
            seed_init: 0,
            rd_q10: 0,
        }
    }
}

/// `NSQ_sample_struct`: one candidate's next sample.
#[derive(Debug, Clone, Copy, Default)]
struct Sample {
    q_q10: i32,
    rd_q10: i32,
    xq_q14: i32,
    lf_ar_q14: i32,
    diff_q14: i32,
    s_ltp_shp_q14: i32,
    lpc_exc_q14: i32,
}

/// The winner's samples still held back, written out: pulses, output and shaping state.
#[allow(clippy::too_many_arguments)]
fn flush_winner(
    dd: &DelDec,
    st: &mut NsqState,
    pulses: &mut [i8; MAX_FRAME_LENGTH],
    pulses_at: usize,
    xq_at: usize,
    smpl_buf_idx: usize,
    decision_delay: usize,
    gain: i32,
    shift: u32,
) {
    let mut last = smpl_buf_idx + decision_delay;
    for i in 0..decision_delay {
        last = (last + DECISION_DELAY - 1) % DECISION_DELAY;
        pulses[pulses_at + i - decision_delay] = rshift_round(dd.q_q10[last], 10) as i8;
        st.xq[xq_at + i - decision_delay] =
            sat16(rshift_round(smulww(dd.xq_q14[last], gain), shift));
        st.s_ltp_shp_q14[st.s_ltp_shp_buf_idx - decision_delay + i] = dd.shape_q14[last];
    }
}

/// `silk_NSQ_del_dec_c`: quantises the frame `x16`; returns the seed index the winning
/// candidate started from, which the frame then codes.
pub(super) fn nsq_del_dec(
    p: &NsqParams<'_>,
    st: &mut NsqState,
    seed: i8,
    x16: &[i16],
    pulses: &mut [i8; MAX_FRAME_LENGTH],
) -> i8 {
    let n_states = p.n_states_delayed_decision.clamp(1, MAX_DEL_DEC_STATES);
    let mut lag = st.lag_prev;
    let mut del_dec = [DelDec::default(); MAX_DEL_DEC_STATES];
    for (k, dd) in del_dec.iter_mut().enumerate().take(n_states) {
        dd.seed = (k as i32 + i32::from(seed)) & 3;
        dd.seed_init = dd.seed;
        dd.rd_q10 = 0;
        dd.lf_ar_q14 = st.s_lf_ar_shp_q14;
        dd.diff_q14 = st.s_diff_shp_q14;
        dd.shape_q14[0] = st.s_ltp_shp_q14[p.ltp_mem_length - 1];
        dd.s_lpc_q14[..NSQ_LPC_BUF_LENGTH].copy_from_slice(&st.s_lpc_q14[..NSQ_LPC_BUF_LENGTH]);
        dd.s_ar2_q14 = st.s_ar2_q14;
    }
    let offset_q10 = i32::from(
        QUANTIZATION_OFFSETS_Q10[(p.signal_type >> 1) as usize][p.quant_offset_type as usize],
    );
    let mut smpl_buf_idx = 0usize;
    let mut decision_delay = DECISION_DELAY.min(p.subfr_length) as i32;
    if p.signal_type == TYPE_VOICED {
        for k in 0..p.nb_subfr {
            decision_delay = decision_delay.min(p.pitch_l[k] - LTP_ORDER as i32 / 2 - 1);
        }
    } else if lag > 0 {
        decision_delay = decision_delay.min(lag - LTP_ORDER as i32 / 2 - 1);
    }
    let decision_delay = decision_delay.max(0) as usize;
    let lsf_interpolation = p.nlsf_interp_coef_q2 != 4;

    let mut s_ltp_q15 = [0i32; 2 * MAX_FRAME_LENGTH];
    let mut s_ltp = [0i16; 2 * MAX_FRAME_LENGTH];
    let mut x_sc_q10 = [0i32; MAX_SUB_FRAME_LENGTH];
    let mut delayed_gain_q10 = [0i32; DECISION_DELAY];
    let mut pxq = p.ltp_mem_length;
    st.s_ltp_shp_buf_idx = p.ltp_mem_length;
    st.s_ltp_buf_idx = p.ltp_mem_length;
    let mut subfr = 0;
    for k in 0..p.nb_subfr {
        let a_q12 = &p.pred_coef_q12[(k >> 1) | usize::from(!lsf_interpolation)];
        let b_q14 = &p.ltp_coef_q14[k * LTP_ORDER..][..LTP_ORDER];
        let ar_shp_q13 = &p.ar_q13[k * MAX_SHAPE_LPC_ORDER..][..MAX_SHAPE_LPC_ORDER];
        let harm_packed =
            (p.harm_shape_gain_q14[k] >> 2) | lshift(p.harm_shape_gain_q14[k] >> 1, 16);
        st.rewhite_flag = false;
        if p.signal_type == TYPE_VOICED {
            lag = p.pitch_l[k];
            if k & (3 - (usize::from(lsf_interpolation) << 1)) == 0 {
                if k == 2 {
                    // Reset the delayed decisions: commit the best candidate so far.
                    let mut rd_min = del_dec[0].rd_q10;
                    let mut winner = 0;
                    for (i, dd) in del_dec.iter().enumerate().take(n_states).skip(1) {
                        if dd.rd_q10 < rd_min {
                            rd_min = dd.rd_q10;
                            winner = i;
                        }
                    }
                    for (i, dd) in del_dec.iter_mut().enumerate().take(n_states) {
                        if i != winner {
                            dd.rd_q10 = dd.rd_q10.wrapping_add(i32::MAX >> 4);
                        }
                    }
                    let dd = del_dec[winner];
                    flush_winner(
                        &dd,
                        st,
                        pulses,
                        k * p.subfr_length,
                        pxq,
                        smpl_buf_idx,
                        decision_delay,
                        p.gains_q16[1],
                        14,
                    );
                    subfr = 0;
                }
                let start_idx =
                    p.ltp_mem_length - lag as usize - p.predict_lpc_order - LTP_ORDER / 2;
                let n = p.ltp_mem_length - start_idx;
                lpc_analysis_filter(
                    &mut s_ltp[start_idx..start_idx + n],
                    &st.xq[start_idx + k * p.subfr_length..][..n],
                    &a_q12[..p.predict_lpc_order],
                );
                st.s_ltp_buf_idx = p.ltp_mem_length;
                st.rewhite_flag = true;
            }
        }
        scale_states(
            p,
            st,
            &mut del_dec[..n_states],
            &x16[k * p.subfr_length..],
            &mut x_sc_q10,
            &s_ltp,
            &mut s_ltp_q15,
            k,
            decision_delay,
        );
        quantizer_del_dec(
            st,
            &mut del_dec[..n_states],
            p,
            &x_sc_q10,
            pulses,
            k * p.subfr_length,
            pxq,
            &mut s_ltp_q15,
            &mut delayed_gain_q10,
            a_q12,
            b_q14,
            ar_shp_q13,
            lag,
            harm_packed,
            k,
            offset_q10,
            subfr,
            &mut smpl_buf_idx,
            decision_delay,
        );
        subfr += 1;
        pxq += p.subfr_length;
    }

    let mut rd_min = del_dec[0].rd_q10;
    let mut winner = 0;
    for (k, dd) in del_dec.iter().enumerate().take(n_states).skip(1) {
        if dd.rd_q10 < rd_min {
            rd_min = dd.rd_q10;
            winner = k;
        }
    }
    let dd = del_dec[winner];
    let gain_q10 = p.gains_q16[p.nb_subfr - 1] >> 6;
    flush_winner(
        &dd,
        st,
        pulses,
        p.nb_subfr * p.subfr_length,
        pxq,
        smpl_buf_idx,
        decision_delay,
        gain_q10,
        8,
    );
    st.s_lpc_q14[..NSQ_LPC_BUF_LENGTH]
        .copy_from_slice(&dd.s_lpc_q14[p.subfr_length..p.subfr_length + NSQ_LPC_BUF_LENGTH]);
    st.s_ar2_q14 = dd.s_ar2_q14;
    st.s_lf_ar_shp_q14 = dd.lf_ar_q14;
    st.s_diff_shp_q14 = dd.diff_q14;
    st.lag_prev = p.pitch_l[p.nb_subfr - 1];
    st.xq
        .copy_within(p.frame_length..p.frame_length + p.ltp_mem_length, 0);
    st.s_ltp_shp_q14
        .copy_within(p.frame_length..p.frame_length + p.ltp_mem_length, 0);
    dd.seed_init as i8
}

/// `silk_noise_shape_quantizer_del_dec`: one subframe.
#[allow(clippy::too_many_arguments)]
fn quantizer_del_dec(
    st: &mut NsqState,
    del_dec: &mut [DelDec],
    p: &NsqParams<'_>,
    x_q10: &[i32],
    pulses: &mut [i8; MAX_FRAME_LENGTH],
    pulses_at: usize,
    xq_at: usize,
    s_ltp_q15: &mut [i32],
    delayed_gain_q10: &mut [i32; DECISION_DELAY],
    a_q12: &[i16],
    b_q14: &[i16],
    ar_shp_q13: &[i16],
    lag: i32,
    harm_packed: i32,
    k_subfr: usize,
    offset_q10: i32,
    subfr: usize,
    smpl_buf_idx: &mut usize,
    decision_delay: usize,
) {
    let n_states = del_dec.len();
    let length = p.subfr_length;
    let order = p.shaping_lpc_order;
    let warping_q16 = p.warping_q16;
    let tilt_q14 = p.tilt_q14[k_subfr];
    let lf_shp_q14 = p.lf_shp_q14[k_subfr];
    let lambda_q10 = p.lambda_q10;
    let gain_q10 = p.gains_q16[k_subfr] >> 6;
    let mut ss = [[Sample::default(); 2]; MAX_DEL_DEC_STATES];
    let mut shp_lag =
        st.s_ltp_shp_buf_idx as isize - lag as isize + HARM_SHAPE_FIR_TAPS as isize / 2;
    let mut pred_lag = st.s_ltp_buf_idx as isize - lag as isize + LTP_ORDER as isize / 2;
    for i in 0..length {
        let ltp_pred_q14 = if p.signal_type == TYPE_VOICED {
            let mut v = 2;
            for t in 0..LTP_ORDER {
                v = smlawb(
                    v,
                    s_ltp_q15[(pred_lag - t as isize) as usize],
                    b_q14[t].into(),
                );
            }
            pred_lag += 1;
            lshift(v, 1)
        } else {
            0
        };
        let n_ltp_q14 = if lag > 0 {
            let sl = |o: isize| st.s_ltp_shp_q14[(shp_lag + o) as usize];
            let mut v = smulwb(add_sat32(sl(0), sl(-2)), harm_packed);
            v = smlawt(v, sl(-1), harm_packed);
            shp_lag += 1;
            sub_lshift32(ltp_pred_q14, v, 2)
        } else {
            0
        };

        for k in 0..n_states {
            let dd = &mut del_dec[k];
            let pss = &mut ss[k];
            dd.seed = silk_rand(dd.seed);
            let lpc_pred_q14 = lshift(
                short_prediction(
                    &dd.s_lpc_q14,
                    NSQ_LPC_BUF_LENGTH - 1 + i,
                    a_q12,
                    p.predict_lpc_order,
                ),
                4,
            );

            // Warped noise shape feedback.
            let mut tmp2 = smlawb(dd.diff_q14, dd.s_ar2_q14[0], warping_q16);
            let mut tmp1 = smlawb(
                dd.s_ar2_q14[0],
                dd.s_ar2_q14[1].wrapping_sub(tmp2),
                warping_q16,
            );
            dd.s_ar2_q14[0] = tmp2;
            let mut n_ar_q14 = (order >> 1) as i32;
            n_ar_q14 = smlawb(n_ar_q14, tmp2, ar_shp_q13[0].into());
            let mut j = 2;
            while j < order {
                tmp2 = smlawb(
                    dd.s_ar2_q14[j - 1],
                    dd.s_ar2_q14[j].wrapping_sub(tmp1),
                    warping_q16,
                );
                dd.s_ar2_q14[j - 1] = tmp1;
                n_ar_q14 = smlawb(n_ar_q14, tmp1, ar_shp_q13[j - 1].into());
                tmp1 = smlawb(
                    dd.s_ar2_q14[j],
                    dd.s_ar2_q14[j + 1].wrapping_sub(tmp2),
                    warping_q16,
                );
                dd.s_ar2_q14[j] = tmp2;
                n_ar_q14 = smlawb(n_ar_q14, tmp2, ar_shp_q13[j].into());
                j += 2;
            }
            dd.s_ar2_q14[order - 1] = tmp1;
            n_ar_q14 = smlawb(n_ar_q14, tmp1, ar_shp_q13[order - 1].into());
            n_ar_q14 = lshift(n_ar_q14, 1);
            n_ar_q14 = smlawb(n_ar_q14, dd.lf_ar_q14, tilt_q14);
            n_ar_q14 = lshift(n_ar_q14, 2);
            let mut n_lf_q14 = smulwb(dd.shape_q14[*smpl_buf_idx], lf_shp_q14);
            n_lf_q14 = smlawt(n_lf_q14, dd.lf_ar_q14, lf_shp_q14);
            n_lf_q14 = lshift(n_lf_q14, 2);

            let t1 = add_sat32(n_ar_q14, n_lf_q14);
            let t2 = n_ltp_q14.wrapping_add(lpc_pred_q14);
            let t1 = rshift_round(sub_sat32(t2, t1), 4);
            let mut r_q10 = x_q10[i].wrapping_sub(t1);
            if dd.seed < 0 {
                r_q10 = r_q10.wrapping_neg();
            }
            r_q10 = r_q10.clamp(-(31 << 10), 30 << 10);

            let (_, [q1_q10, rd1_q20, q2_q10, rd2_q20]) =
                quantize_sample(r_q10, offset_q10, lambda_q10);
            let rd1_q10 = rd1_q20 >> 10;
            let rd2_q10 = rd2_q20 >> 10;
            if rd1_q10 < rd2_q10 {
                pss[0].rd_q10 = dd.rd_q10.wrapping_add(rd1_q10);
                pss[1].rd_q10 = dd.rd_q10.wrapping_add(rd2_q10);
                pss[0].q_q10 = q1_q10;
                pss[1].q_q10 = q2_q10;
            } else {
                pss[0].rd_q10 = dd.rd_q10.wrapping_add(rd2_q10);
                pss[1].rd_q10 = dd.rd_q10.wrapping_add(rd1_q10);
                pss[0].q_q10 = q2_q10;
                pss[1].q_q10 = q1_q10;
            }
            for s in pss.iter_mut() {
                let mut exc_q14 = lshift(s.q_q10, 4);
                if dd.seed < 0 {
                    exc_q14 = exc_q14.wrapping_neg();
                }
                let lpc_exc_q14 = exc_q14.wrapping_add(ltp_pred_q14);
                let xq_q14 = lpc_exc_q14.wrapping_add(lpc_pred_q14);
                s.diff_q14 = xq_q14.wrapping_sub(lshift(x_q10[i], 4));
                let s_lf_ar_shp_q14 = s.diff_q14.wrapping_sub(n_ar_q14);
                s.s_ltp_shp_q14 = sub_sat32(s_lf_ar_shp_q14, n_lf_q14);
                s.lf_ar_q14 = s_lf_ar_shp_q14;
                s.lpc_exc_q14 = lpc_exc_q14;
                s.xq_q14 = xq_q14;
            }
        }

        *smpl_buf_idx = (*smpl_buf_idx + DECISION_DELAY - 1) % DECISION_DELAY;
        let last = (*smpl_buf_idx + decision_delay) % DECISION_DELAY;

        let mut rd_min = ss[0][0].rd_q10;
        let mut winner = 0;
        for k in 1..n_states {
            if ss[k][0].rd_q10 < rd_min {
                rd_min = ss[k][0].rd_q10;
                winner = k;
            }
        }
        let winner_rand_state = del_dec[winner].rand_state[last];
        for k in 0..n_states {
            if del_dec[k].rand_state[last] != winner_rand_state {
                ss[k][0].rd_q10 = ss[k][0].rd_q10.wrapping_add(i32::MAX >> 4);
                ss[k][1].rd_q10 = ss[k][1].rd_q10.wrapping_add(i32::MAX >> 4);
            }
        }
        let mut rd_max = ss[0][0].rd_q10;
        let mut rd_min = ss[0][1].rd_q10;
        let mut rd_max_ind = 0;
        let mut rd_min_ind = 0;
        for k in 1..n_states {
            if ss[k][0].rd_q10 > rd_max {
                rd_max = ss[k][0].rd_q10;
                rd_max_ind = k;
            }
            if ss[k][1].rd_q10 < rd_min {
                rd_min = ss[k][1].rd_q10;
                rd_min_ind = k;
            }
        }
        if rd_min < rd_max {
            // The C copies from the i-th word on; the words before are never read again.
            del_dec[rd_max_ind] = del_dec[rd_min_ind];
            ss[rd_max_ind][0] = ss[rd_min_ind][1];
        }

        let dd = &del_dec[winner];
        if subfr > 0 || i >= decision_delay {
            pulses[pulses_at + i - decision_delay] = rshift_round(dd.q_q10[last], 10) as i8;
            st.xq[xq_at + i - decision_delay] = sat16(rshift_round(
                smulww(dd.xq_q14[last], delayed_gain_q10[last]),
                8,
            ));
            st.s_ltp_shp_q14[st.s_ltp_shp_buf_idx - decision_delay] = dd.shape_q14[last];
            s_ltp_q15[st.s_ltp_buf_idx - decision_delay] = dd.pred_q15[last];
        }
        st.s_ltp_shp_buf_idx += 1;
        st.s_ltp_buf_idx += 1;

        for k in 0..n_states {
            let dd = &mut del_dec[k];
            let s = &ss[k][0];
            dd.lf_ar_q14 = s.lf_ar_q14;
            dd.diff_q14 = s.diff_q14;
            dd.s_lpc_q14[NSQ_LPC_BUF_LENGTH + i] = s.xq_q14;
            dd.xq_q14[*smpl_buf_idx] = s.xq_q14;
            dd.q_q10[*smpl_buf_idx] = s.q_q10;
            dd.pred_q15[*smpl_buf_idx] = lshift(s.lpc_exc_q14, 1);
            dd.shape_q14[*smpl_buf_idx] = s.s_ltp_shp_q14;
            dd.seed = dd.seed.wrapping_add(rshift_round(s.q_q10, 10));
            dd.rand_state[*smpl_buf_idx] = dd.seed;
            dd.rd_q10 = s.rd_q10;
        }
        delayed_gain_q10[*smpl_buf_idx] = gain_q10;
    }
    for dd in del_dec.iter_mut() {
        dd.s_lpc_q14
            .copy_within(length..length + NSQ_LPC_BUF_LENGTH, 0);
    }
}

/// `silk_nsq_del_dec_scale_states`.
#[allow(clippy::too_many_arguments)]
fn scale_states(
    p: &NsqParams<'_>,
    st: &mut NsqState,
    del_dec: &mut [DelDec],
    x16: &[i16],
    x_sc_q10: &mut [i32],
    s_ltp: &[i16],
    s_ltp_q15: &mut [i32],
    subfr: usize,
    decision_delay: usize,
) {
    let lag = p.pitch_l[subfr] as usize;
    let mut inv_gain_q31 = inverse32_varq(p.gains_q16[subfr].max(1), 47);
    let inv_gain_q26 = rshift_round(inv_gain_q31, 5);
    for i in 0..p.subfr_length {
        x_sc_q10[i] = smulww(x16[i].into(), inv_gain_q26);
    }
    if st.rewhite_flag {
        if subfr == 0 {
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
            for i in st.s_ltp_buf_idx - lag - LTP_ORDER / 2..st.s_ltp_buf_idx - decision_delay {
                s_ltp_q15[i] = smulww(gain_adj_q16, s_ltp_q15[i]);
            }
        }
        for dd in del_dec.iter_mut() {
            dd.lf_ar_q14 = smulww(gain_adj_q16, dd.lf_ar_q14);
            dd.diff_q14 = smulww(gain_adj_q16, dd.diff_q14);
            for v in dd.s_lpc_q14[..NSQ_LPC_BUF_LENGTH].iter_mut() {
                *v = smulww(gain_adj_q16, *v);
            }
            for v in dd.s_ar2_q14.iter_mut() {
                *v = smulww(gain_adj_q16, *v);
            }
            for i in 0..DECISION_DELAY {
                dd.pred_q15[i] = smulww(gain_adj_q16, dd.pred_q15[i]);
                dd.shape_q14[i] = smulww(gain_adj_q16, dd.shape_q14[i]);
            }
        }
        st.prev_gain_q16 = p.gains_q16[subfr];
    }
}
