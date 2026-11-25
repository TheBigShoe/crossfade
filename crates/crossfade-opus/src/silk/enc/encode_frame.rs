//! One SILK frame through the encoder (libopus `silk/fixed/encode_frame_FIX.c`): voice
//! activity, analysis, then quantisation and entropy coding in a loop that adjusts the gains
//! until the frame meets its bit budget; and the frame's LBRR copy.

use super::analysis::{find_pitch_lags, find_pred_coefs, process_gains};
use super::encode_indices::{IndexCoder, encode_indices};
use super::encode_pulses::encode_pulses;
use super::fix::{fix_f32, lshift_sat32, smulwb};
use super::lp_cutoff::lp_variable_cutoff;
use super::noise_shape::noise_shape_analysis;
use super::nsq::{NsqParams, nsq};
use super::nsq_del_dec::nsq_del_dec;
use super::quant::{gains_id, gains_quant};
use super::state::{ChannelEncoder, EncoderControl, LA_SHAPE_MS, NsqState, TYPE_UNVOICED};
use super::vad::vad_get_sa_q8;
use crate::range_enc::RangeEncoder;
use crate::silk::decode_parameters::gains_dequant;
use crate::silk::state::{CondCoding, MAX_FRAME_LENGTH, SideInfoIndices, TYPE_NO_VOICE_ACTIVITY};

const SPEECH_ACTIVITY_DTX_THRES: f32 = 0.05;
const LBRR_SPEECH_ACTIVITY_THRES: f32 = 0.3;
const NB_SPEECH_FRAMES_BEFORE_DTX: i32 = 10;
const MAX_CONSECUTIVE_DTX: i32 = 20;
const N_LEVELS_QGAIN: i8 = 64;
const LA_PITCH_MAX: usize = 2 * 16;

/// `silk_encode_do_VAD_FIX`: speech activity, then the frame's VAD flag and a first signal
/// type (no activity or unvoiced). `activity` is the Opus layer's own decision (`Some(false)`
/// lowers SILK's activity below the threshold).
pub(super) fn encode_do_vad(st: &mut ChannelEncoder, activity: Option<bool>) {
    let threshold = fix_f32(SPEECH_ACTIVITY_DTX_THRES, 8);
    let input = st.input_buf;
    vad_get_sa_q8(st, &input[1..]);
    if activity == Some(false) && st.speech_activity_q8 >= threshold {
        st.speech_activity_q8 = threshold - 1;
    }
    let n = st.n_frames_encoded;
    if st.speech_activity_q8 < threshold {
        st.indices.signal_type = TYPE_NO_VOICE_ACTIVITY as i8;
        st.no_speech_counter += 1;
        if st.no_speech_counter <= NB_SPEECH_FRAMES_BEFORE_DTX {
            st.in_dtx = false;
        } else if st.no_speech_counter > MAX_CONSECUTIVE_DTX + NB_SPEECH_FRAMES_BEFORE_DTX {
            st.no_speech_counter = NB_SPEECH_FRAMES_BEFORE_DTX;
            st.in_dtx = false;
        }
        st.vad_flags[n] = false;
    } else {
        st.no_speech_counter = 0;
        st.in_dtx = false;
        st.indices.signal_type = TYPE_UNVOICED as i8;
        st.vad_flags[n] = true;
    }
}

fn nsq_params<'a>(
    st: &ChannelEncoder,
    ix: &SideInfoIndices,
    ctrl: &'a EncoderControl,
) -> NsqParams<'a> {
    NsqParams {
        ltp_mem_length: st.ltp_mem_length,
        frame_length: st.frame_length,
        subfr_length: st.subfr_length,
        nb_subfr: st.nb_subfr,
        predict_lpc_order: st.predict_lpc_order,
        shaping_lpc_order: st.shaping_lpc_order,
        warping_q16: st.warping_q16,
        n_states_delayed_decision: st.n_states_delayed_decision,
        signal_type: ix.signal_type.into(),
        quant_offset_type: ix.quant_offset_type.into(),
        nlsf_interp_coef_q2: ix.nlsf_interp_coef_q2.into(),
        pred_coef_q12: &ctrl.pred_coef_q12,
        ltp_coef_q14: &ctrl.ltp_coef_q14,
        ar_q13: &ctrl.ar_q13,
        harm_shape_gain_q14: &ctrl.harm_shape_gain_q14,
        tilt_q14: &ctrl.tilt_q14,
        lf_shp_q14: &ctrl.lf_shp_q14,
        gains_q16: &ctrl.gains_q16,
        pitch_l: &ctrl.pitch_l,
        lambda_q10: ctrl.lambda_q10,
        ltp_scale_q14: ctrl.ltp_scale_q14,
    }
}

/// `silk_NSQ` or `silk_NSQ_del_dec`, as the complexity asks; the delayed decision may change
/// the seed.
fn quantize(
    st: &ChannelEncoder,
    ix: &mut SideInfoIndices,
    ctrl: &EncoderControl,
    nsq_state: &mut NsqState,
    x_frame: &[i16],
    pulses: &mut [i8; MAX_FRAME_LENGTH],
) {
    let p = nsq_params(st, ix, ctrl);
    if st.n_states_delayed_decision > 1 || st.warping_q16 > 0 {
        ix.seed = nsq_del_dec(&p, nsq_state, ix.seed, x_frame, pulses);
    } else {
        nsq(&p, nsq_state, ix.seed, x_frame, pulses);
    }
}

fn index_coder(st: &mut ChannelEncoder) -> (IndexCoder<'_>, &SideInfoIndices) {
    let ChannelEncoder {
        nb_subfr,
        fs_khz,
        nlsf_cb,
        pitch_lag_low_bits_icdf,
        pitch_contour_icdf,
        ec_prev_signal_type,
        ec_prev_lag_index,
        indices,
        ..
    } = st;
    (
        IndexCoder {
            nb_subfr: *nb_subfr,
            fs_khz: *fs_khz,
            nlsf_cb,
            pitch_lag_low_bits_icdf,
            pitch_contour_icdf,
            ec_prev_signal_type,
            ec_prev_lag_index,
        },
        indices,
    )
}

/// `silk_encode_indices` then `silk_encode_pulses` for the frame's regular data.
fn encode_payload(st: &mut ChannelEncoder, enc: &mut RangeEncoder, cond_coding: CondCoding) {
    let (mut c, ix) = index_coder(st);
    let ix = ix.clone();
    encode_indices(&mut c, &ix, enc, false, cond_coding);
    let frame_length = st.frame_length;
    encode_pulses(
        enc,
        ix.signal_type.into(),
        ix.quant_offset_type.into(),
        &mut st.pulses,
        frame_length,
    );
}

/// `silk_encode_frame_FIX`: codes the frame in `input_buf` into `enc`, within `max_bits`;
/// returns the payload bytes so far (`pnBytesOut`), 0 in prefill.
pub(super) fn encode_frame(
    st: &mut ChannelEncoder,
    enc: &mut RangeEncoder,
    cond_coding: CondCoding,
    max_bits: i32,
    use_cbr: bool,
) -> usize {
    let bits_margin = if use_cbr { 5 } else { max_bits / 4 };
    let mut ctrl = EncoderControl::default();
    st.indices.seed = (st.frame_counter & 3) as i8;
    st.frame_counter = st.frame_counter.wrapping_add(1);

    let x_frame = st.ltp_mem_length;
    let frame_length = st.frame_length;
    {
        let (lp, buf) = (&mut st.s_lp, &mut st.input_buf);
        lp_variable_cutoff(lp, &mut buf[1..1 + frame_length]);
    }
    let at = x_frame + LA_SHAPE_MS * st.fs_khz as usize;
    st.x_buf[at..at + frame_length].copy_from_slice(&st.input_buf[1..1 + frame_length]);

    if !st.prefill_flag {
        let mut res_pitch = [0i16; LA_PITCH_MAX + 2 * MAX_FRAME_LENGTH];
        let x_buf = st.x_buf;
        find_pitch_lags(st, &mut ctrl, &mut res_pitch, &x_buf);
        let res_pitch_frame = st.ltp_mem_length;
        noise_shape_analysis(
            st,
            &mut ctrl,
            &res_pitch[res_pitch_frame..],
            &x_buf,
            x_frame,
        );
        find_pred_coefs(st, &mut ctrl, &res_pitch, &x_buf, x_frame, cond_coding);
        process_gains(st, &mut ctrl, cond_coding);
        lbrr_encode(st, &mut ctrl, &x_buf[x_frame..], cond_coding);

        rate_loop(
            st,
            &mut ctrl,
            enc,
            &x_buf[x_frame..],
            cond_coding,
            max_bits,
            use_cbr,
            bits_margin,
        );
    }

    let n = st.ltp_mem_length + LA_SHAPE_MS * st.fs_khz as usize;
    st.x_buf.copy_within(frame_length..frame_length + n, 0);
    if st.prefill_flag {
        return 0;
    }
    st.prev_lag = ctrl.pitch_l[st.nb_subfr - 1];
    st.prev_signal_type = st.indices.signal_type.into();
    st.first_frame_after_reset = false;
    ((enc.tell() + 7) >> 3) as usize
}

/// The loop over quantisation and entropy coding that controls the frame's bitrate.
#[allow(clippy::too_many_arguments)]
fn rate_loop(
    st: &mut ChannelEncoder,
    ctrl: &mut EncoderControl,
    enc: &mut RangeEncoder,
    x_frame: &[i16],
    cond_coding: CondCoding,
    max_bits: i32,
    use_cbr: bool,
    bits_margin: i32,
) {
    const MAX_ITER: i32 = 6;
    let nb_subfr = st.nb_subfr;
    let mut gain_mult_q8: i32 = 1 << 8;
    let mut found_lower = false;
    let mut found_upper = false;
    let mut gains_id_cur = gains_id(&st.indices.gains_indices, nb_subfr);
    let mut gains_id_lower = -1;
    let mut gains_id_upper = -1;
    let (mut n_bits_lower, mut n_bits_upper) = (0, 0);
    let (mut gain_mult_lower, mut gain_mult_upper) = (0, 0);
    let mut last_gain_index_copy2 = 0i8;
    let mut gain_lock = [false; 4];
    let mut best_gain_mult = [0i32; 4];
    let mut best_sum = [0i32; 4];

    let enc_copy = enc.clone();
    let nsq_copy = st.s_nsq.clone();
    let seed_copy = st.indices.seed;
    let ec_prev_lag_index_copy = st.ec_prev_lag_index;
    let ec_prev_signal_type_copy = st.ec_prev_signal_type;
    let mut enc_copy2: Option<RangeEncoder> = None;
    let mut nsq_copy2 = st.s_nsq.clone();

    let mut iter = 0;
    loop {
        let n_bits;
        if gains_id_cur == gains_id_lower {
            n_bits = n_bits_lower;
        } else if gains_id_cur == gains_id_upper {
            n_bits = n_bits_upper;
        } else {
            if iter > 0 {
                *enc = enc_copy.clone();
                st.s_nsq = nsq_copy.clone();
                st.indices.seed = seed_copy;
                st.ec_prev_lag_index = ec_prev_lag_index_copy;
                st.ec_prev_signal_type = ec_prev_signal_type_copy;
            }
            {
                let mut ix = st.indices.clone();
                let mut nsq_state = std::mem::take(&mut st.s_nsq);
                let mut pulses = st.pulses;
                quantize(st, &mut ix, ctrl, &mut nsq_state, x_frame, &mut pulses);
                st.s_nsq = nsq_state;
                st.pulses = pulses;
                st.indices = ix;
            }
            if iter == MAX_ITER && !found_lower {
                enc_copy2 = Some(enc.clone());
            }
            encode_payload(st, enc, cond_coding);
            let mut nb = enc.tell();
            if iter == MAX_ITER && !found_lower && nb > max_bits {
                // Still too many bits: keep the last frame's gains and drop the pulses.
                if let Some(c) = &enc_copy2 {
                    *enc = c.clone();
                }
                st.s_shape.last_gain_index = ctrl.last_gain_index_prev;
                for g in st.indices.gains_indices[..nb_subfr].iter_mut() {
                    *g = 4;
                }
                if cond_coding != CondCoding::Conditionally {
                    st.indices.gains_indices[0] = ctrl.last_gain_index_prev;
                }
                st.ec_prev_lag_index = ec_prev_lag_index_copy;
                st.ec_prev_signal_type = ec_prev_signal_type_copy;
                st.pulses[..st.frame_length].fill(0);
                encode_payload(st, enc, cond_coding);
                nb = enc.tell();
            }
            n_bits = nb;
            if !use_cbr && iter == 0 && n_bits <= max_bits {
                break;
            }
        }

        if iter == MAX_ITER {
            if found_lower && (gains_id_cur == gains_id_lower || n_bits > max_bits) {
                // Back to the earlier iteration that met the budget.
                if let Some(c) = &enc_copy2 {
                    *enc = c.clone();
                }
                st.s_nsq = nsq_copy2.clone();
                st.s_shape.last_gain_index = last_gain_index_copy2;
            }
            break;
        }

        if n_bits > max_bits {
            if !found_lower && iter >= 2 {
                // Trade rate for distortion; forget the earlier "upper" results.
                ctrl.lambda_q10 += ctrl.lambda_q10 >> 1;
                found_upper = false;
                gains_id_upper = -1;
            } else {
                found_upper = true;
                n_bits_upper = n_bits;
                gain_mult_upper = gain_mult_q8;
                gains_id_upper = gains_id_cur;
            }
        } else if n_bits < max_bits - bits_margin {
            found_lower = true;
            n_bits_lower = n_bits;
            gain_mult_lower = gain_mult_q8;
            if gains_id_cur != gains_id_lower {
                gains_id_lower = gains_id_cur;
                enc_copy2 = Some(enc.clone());
                nsq_copy2 = st.s_nsq.clone();
                last_gain_index_copy2 = st.s_shape.last_gain_index;
            }
        } else {
            // Close enough.
            break;
        }

        if !found_lower && n_bits > max_bits {
            for i in 0..nb_subfr {
                let sl = st.subfr_length;
                let sum: i32 = st.pulses[i * sl..(i + 1) * sl]
                    .iter()
                    .map(|&p| i32::from(p).abs())
                    .sum();
                if iter == 0 || (sum < best_sum[i] && !gain_lock[i]) {
                    best_sum[i] = sum;
                    best_gain_mult[i] = gain_mult_q8;
                } else {
                    gain_lock[i] = true;
                }
            }
        }
        if !(found_lower && found_upper) {
            gain_mult_q8 = if n_bits > max_bits {
                (gain_mult_q8 * 3 / 2).min(1024)
            } else {
                (gain_mult_q8 * 4 / 5).max(64)
            };
        } else {
            gain_mult_q8 = gain_mult_lower
                + (gain_mult_upper - gain_mult_lower) * (max_bits - n_bits_lower)
                    / (n_bits_upper - n_bits_lower);
            let hi = gain_mult_lower + ((gain_mult_upper - gain_mult_lower) >> 2);
            let lo = gain_mult_upper - ((gain_mult_upper - gain_mult_lower) >> 2);
            if gain_mult_q8 > hi {
                gain_mult_q8 = hi;
            } else if gain_mult_q8 < lo {
                gain_mult_q8 = lo;
            }
        }
        for i in 0..nb_subfr {
            let m = if gain_lock[i] {
                best_gain_mult[i]
            } else {
                gain_mult_q8
            };
            // The C keeps the multiplier in an opus_int16.
            ctrl.gains_q16[i] = lshift_sat32(smulwb(ctrl.gains_unq_q16[i], i32::from(m as i16)), 8);
        }
        st.s_shape.last_gain_index = ctrl.last_gain_index_prev;
        gains_quant(
            &mut st.indices.gains_indices,
            &mut ctrl.gains_q16,
            &mut st.s_shape.last_gain_index,
            cond_coding == CondCoding::Conditionally,
            nb_subfr,
        );
        gains_id_cur = gains_id(&st.indices.gains_indices, nb_subfr);
        iter += 1;
    }
}

/// `silk_LBRR_encode_FIX`: the frame again at a lower rate, for the next packet to carry.
fn lbrr_encode(
    st: &mut ChannelEncoder,
    ctrl: &mut EncoderControl,
    x16: &[i16],
    cond_coding: CondCoding,
) {
    let n = st.n_frames_encoded;
    if !(st.lbrr_enabled && st.speech_activity_q8 > fix_f32(LBRR_SPEECH_ACTIVITY_THRES, 8)) {
        return;
    }
    st.lbrr_flags[n] = true;
    let mut nsq_lbrr = st.s_nsq.clone();
    let mut ix = st.indices.clone();
    let temp_gains_q16 = ctrl.gains_q16;
    if n == 0 || !st.lbrr_flags[n - 1] {
        st.lbrr_prev_last_gain_index = st.s_shape.last_gain_index;
        ix.gains_indices[0] =
            (ix.gains_indices[0] + st.lbrr_gain_increases as i8).min(N_LEVELS_QGAIN - 1);
    }
    gains_dequant(
        &mut ctrl.gains_q16,
        &ix.gains_indices,
        &mut st.lbrr_prev_last_gain_index,
        cond_coding == CondCoding::Conditionally,
        st.nb_subfr,
    );
    let mut pulses = [0i8; MAX_FRAME_LENGTH];
    quantize(st, &mut ix, ctrl, &mut nsq_lbrr, x16, &mut pulses);
    st.pulses_lbrr[n] = pulses;
    st.indices_lbrr[n] = ix;
    ctrl.gains_q16 = temp_gains_q16;
}

/// The LBRR frame `i`, written (`silk_encode_indices` with `encode_LBRR`, then its pulses).
pub(super) fn encode_lbrr_frame(
    st: &mut ChannelEncoder,
    enc: &mut RangeEncoder,
    i: usize,
    cond_coding: CondCoding,
) {
    let ix = st.indices_lbrr[i].clone();
    let (mut c, _) = index_coder(st);
    encode_indices(&mut c, &ix, enc, true, cond_coding);
    let frame_length = st.frame_length;
    let mut pulses = st.pulses_lbrr[i];
    encode_pulses(
        enc,
        ix.signal_type.into(),
        ix.quant_offset_type.into(),
        &mut pulses,
        frame_length,
    );
}
