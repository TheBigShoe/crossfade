//! The encoder's configuration (libopus `silk/control_codec.c`, `control_audio_bandwidth.c`,
//! `control_SNR.c`, `check_control_input.c`): the internal rate and its switching, frame
//! sizes, complexity, LBRR and the target SNR for a bitrate.

use super::EncControl;
use super::fix::{fix_f64, smulwb};
use super::lp_cutoff::TRANSITION_FRAMES;
use super::pitch::SILK_PE_MIN_COMPLEX;
use super::resampler_enc::DownResampler;
use super::state::{ChannelEncoder, LA_SHAPE_MS, LpState, NsqState, ShapeState};
use crate::error::{Error, Result};
use crate::silk::resampler::Resampler;
use crate::silk::state::{
    MAX_FRAME_LENGTH, MAX_LPC_ORDER, MAX_NB_SUBFR, MIN_LPC_ORDER, TYPE_NO_VOICE_ACTIVITY,
};
use crate::silk::tables::{
    PITCH_CONTOUR_10_MS_ICDF, PITCH_CONTOUR_10_MS_NB_ICDF, PITCH_CONTOUR_ICDF,
    PITCH_CONTOUR_NB_ICDF, UNIFORM4_ICDF, UNIFORM6_ICDF, UNIFORM8_ICDF,
};
use crate::silk::tables_nlsf::{NLSF_CB_NB_MB, NLSF_CB_WB};

const SILK_PE_MID_COMPLEX: usize = 1;
const SILK_PE_MAX_COMPLEX: usize = 2;
const FIND_PITCH_LPC_WIN_MS: usize = 20 + (2 << 1);
const FIND_PITCH_LPC_WIN_MS_2_SF: usize = 10 + (2 << 1);
const WARPING_MULTIPLIER: f32 = 0.015;

static TARGET_RATE_NB_21: [u8; 107] = [
    0, 15, 39, 52, 61, 68, 74, 79, 84, 88, 92, 95, 99, 102, 105, 108, 111, 114, 117, 119, 122, 124,
    126, 129, 131, 133, 135, 137, 139, 142, 143, 145, 147, 149, 151, 153, 155, 157, 158, 160, 162,
    163, 165, 167, 168, 170, 171, 173, 174, 176, 177, 179, 180, 182, 183, 185, 186, 187, 189, 190,
    192, 193, 194, 196, 197, 199, 200, 201, 203, 204, 205, 207, 208, 209, 211, 212, 213, 215, 216,
    217, 219, 220, 221, 223, 224, 225, 227, 228, 230, 231, 232, 234, 235, 236, 238, 239, 241, 242,
    243, 245, 246, 248, 249, 250, 252, 253, 255,
];
static TARGET_RATE_MB_21: [u8; 155] = [
    0, 0, 28, 43, 52, 59, 65, 70, 74, 78, 81, 85, 87, 90, 93, 95, 98, 100, 102, 105, 107, 109, 111,
    113, 115, 116, 118, 120, 122, 123, 125, 127, 128, 130, 131, 133, 134, 136, 137, 138, 140, 141,
    143, 144, 145, 147, 148, 149, 151, 152, 153, 154, 156, 157, 158, 159, 160, 162, 163, 164, 165,
    166, 167, 168, 169, 171, 172, 173, 174, 175, 176, 177, 178, 179, 180, 181, 182, 183, 184, 185,
    186, 187, 188, 188, 189, 190, 191, 192, 193, 194, 195, 196, 197, 198, 199, 200, 201, 202, 203,
    203, 204, 205, 206, 207, 208, 209, 210, 211, 212, 213, 214, 214, 215, 216, 217, 218, 219, 220,
    221, 222, 223, 224, 224, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 235, 236, 236, 237,
    238, 239, 240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255,
];
static TARGET_RATE_WB_21: [u8; 191] = [
    0, 0, 0, 8, 29, 41, 49, 56, 62, 66, 70, 74, 77, 80, 83, 86, 88, 91, 93, 95, 97, 99, 101, 103,
    105, 107, 108, 110, 112, 113, 115, 116, 118, 119, 121, 122, 123, 125, 126, 127, 129, 130, 131,
    132, 134, 135, 136, 137, 138, 140, 141, 142, 143, 144, 145, 146, 147, 148, 149, 150, 151, 152,
    153, 154, 156, 157, 158, 159, 159, 160, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 171,
    171, 172, 173, 174, 175, 176, 177, 177, 178, 179, 180, 181, 181, 182, 183, 184, 185, 185, 186,
    187, 188, 189, 189, 190, 191, 192, 192, 193, 194, 195, 195, 196, 197, 198, 198, 199, 200, 200,
    201, 202, 203, 203, 204, 205, 206, 206, 207, 208, 209, 209, 210, 211, 211, 212, 213, 214, 214,
    215, 216, 216, 217, 218, 219, 219, 220, 221, 221, 222, 223, 224, 224, 225, 226, 226, 227, 228,
    229, 229, 230, 231, 232, 232, 233, 234, 234, 235, 236, 237, 237, 238, 239, 240, 240, 241, 242,
    243, 243, 244, 245, 246, 246, 247, 248, 249, 249, 250, 251, 252, 253, 255,
];

/// `check_control_input`, for the configurations this encoder takes.
pub(super) fn check_control_input(c: &EncControl) -> Result<()> {
    let rate_ok = |r: i32| matches!(r, 8000 | 12000 | 16000);
    if c.api_sample_rate != 48_000
        || !rate_ok(c.desired_internal_sample_rate)
        || !rate_ok(c.max_internal_sample_rate)
        || !rate_ok(c.min_internal_sample_rate)
        || c.min_internal_sample_rate > c.desired_internal_sample_rate
        || c.max_internal_sample_rate < c.desired_internal_sample_rate
        || c.min_internal_sample_rate > c.max_internal_sample_rate
    {
        return Err(Error::BadArgument("silk: sample rates"));
    }
    if !matches!(c.payload_size_ms, 10 | 20 | 40 | 60) {
        return Err(Error::BadArgument("silk: payload size"));
    }
    if !(0..=100).contains(&c.packet_loss_percentage) {
        return Err(Error::BadArgument("silk: packet loss"));
    }
    if !(1..=2).contains(&c.n_channels_api)
        || !(1..=2).contains(&c.n_channels_internal)
        || c.n_channels_internal > c.n_channels_api
    {
        return Err(Error::BadArgument("silk: channels"));
    }
    if !(0..=10).contains(&c.complexity) {
        return Err(Error::BadArgument("silk: complexity"));
    }
    Ok(())
}

/// `silk_control_SNR`: the target SNR for a bitrate, from tables measured on the encoder.
pub(super) fn control_snr(st: &mut ChannelEncoder, target_rate_bps: i32) {
    st.target_rate_bps = target_rate_bps;
    let mut rate = target_rate_bps;
    if st.nb_subfr == 2 {
        rate -= 2000 + st.fs_khz / 16;
    }
    let table: &[u8] = match st.fs_khz {
        8 => &TARGET_RATE_NB_21,
        12 => &TARGET_RATE_MB_21,
        _ => &TARGET_RATE_WB_21,
    };
    let id = ((rate + 200) / 400 - 10).min(table.len() as i32 - 1);
    st.snr_db_q7 = if id <= 0 {
        0
    } else {
        i32::from(table[id as usize]) * 21
    };
}

/// `silk_control_encoder`: applies the control to one channel (once per packet).
pub(super) fn control_encoder(
    st: &mut ChannelEncoder,
    c: &mut EncControl,
    allow_bw_switch: bool,
    channel_nb: usize,
    force_fs_khz: i32,
) {
    st.use_dtx = c.use_dtx;
    st.use_cbr = c.use_cbr;
    st.api_fs_hz = c.api_sample_rate;
    st.max_internal_fs_hz = c.max_internal_sample_rate;
    st.min_internal_fs_hz = c.min_internal_sample_rate;
    st.desired_internal_fs_hz = c.desired_internal_sample_rate;
    st.n_channels_api = c.n_channels_api;
    st.n_channels_internal = c.n_channels_internal;
    st.allow_bandwidth_switch = allow_bw_switch;
    st.channel_nb = channel_nb;
    if st.controlled_since_last_payload && !st.prefill_flag {
        // The API rate never changes here: nothing to redo mid-packet.
        return;
    }
    let mut fs_khz = control_audio_bandwidth(st, c);
    if force_fs_khz != 0 {
        fs_khz = force_fs_khz;
    }
    setup_resamplers(st, fs_khz);
    setup_fs(st, fs_khz, c.payload_size_ms);
    setup_complexity(st, c.complexity);
    st.packet_loss_perc = c.packet_loss_percentage;
    setup_lbrr(st, c);
    st.controlled_since_last_payload = true;
}

/// `silk_setup_resamplers`: on a rate change, the buffered signal is resampled to the new
/// rate through the API rate, so the analysis keeps its history.
fn setup_resamplers(st: &mut ChannelEncoder, fs_khz: i32) {
    if st.fs_khz != fs_khz || st.prev_api_fs_hz != st.api_fs_hz {
        if st.fs_khz == 0 {
            st.resampler = DownResampler::new(fs_khz * 1000);
        } else {
            let buf_length_ms = ((st.nb_subfr * 5) << 1) + LA_SHAPE_MS;
            let old_buf_samples = buf_length_ms * st.fs_khz as usize;
            let mut up = Resampler::new(st.fs_khz * 1000);
            let api_buf_samples = buf_length_ms * 48;
            let mut x_api = [0i16; (2 * MAX_FRAME_LENGTH / 16 + LA_SHAPE_MS) * 48];
            up.process(&mut x_api[..api_buf_samples], &st.x_buf[..old_buf_samples]);
            st.resampler = DownResampler::new(fs_khz * 1000);
            let new_buf_samples = buf_length_ms * fs_khz as usize;
            st.resampler
                .process(&mut st.x_buf[..new_buf_samples], &x_api[..api_buf_samples]);
        }
    }
    st.prev_api_fs_hz = st.api_fs_hz;
}

/// `silk_setup_fs`: frame sizes for the packet size and internal rate; a rate change resets
/// much of the state.
fn setup_fs(st: &mut ChannelEncoder, fs_khz: i32, packet_size_ms: i32) {
    if packet_size_ms != st.packet_size_ms {
        if packet_size_ms <= 10 {
            st.n_frames_per_packet = 1;
            st.nb_subfr = if packet_size_ms == 10 { 2 } else { 1 };
            st.frame_length = (packet_size_ms * fs_khz) as usize;
            st.pitch_lpc_win_length = FIND_PITCH_LPC_WIN_MS_2_SF * fs_khz as usize;
            st.pitch_contour_icdf = if st.fs_khz == 8 {
                &PITCH_CONTOUR_10_MS_NB_ICDF
            } else {
                &PITCH_CONTOUR_10_MS_ICDF
            };
        } else {
            st.n_frames_per_packet = (packet_size_ms / 20) as usize;
            st.nb_subfr = MAX_NB_SUBFR;
            st.frame_length = 20 * fs_khz as usize;
            st.pitch_lpc_win_length = FIND_PITCH_LPC_WIN_MS * fs_khz as usize;
            st.pitch_contour_icdf = if st.fs_khz == 8 {
                &PITCH_CONTOUR_NB_ICDF
            } else {
                &PITCH_CONTOUR_ICDF
            };
        }
        st.packet_size_ms = packet_size_ms;
        st.target_rate_bps = 0;
    }
    if st.fs_khz != fs_khz {
        st.s_shape = ShapeState::default();
        st.s_nsq = NsqState::default();
        st.prev_nlsfq_q15 = [0; MAX_LPC_ORDER];
        st.s_lp.in_lp_state = [0; 2];
        st.input_buf_ix = 0;
        st.n_frames_encoded = 0;
        st.target_rate_bps = 0;
        st.prev_lag = 100;
        st.first_frame_after_reset = true;
        st.s_shape.last_gain_index = 10;
        st.s_nsq.lag_prev = 100;
        st.s_nsq.prev_gain_q16 = 65536;
        st.prev_signal_type = TYPE_NO_VOICE_ACTIVITY;
        st.fs_khz = fs_khz;
        st.pitch_contour_icdf = match (fs_khz == 8, st.nb_subfr == MAX_NB_SUBFR) {
            (true, true) => &PITCH_CONTOUR_NB_ICDF,
            (true, false) => &PITCH_CONTOUR_10_MS_NB_ICDF,
            (false, true) => &PITCH_CONTOUR_ICDF,
            (false, false) => &PITCH_CONTOUR_10_MS_ICDF,
        };
        if fs_khz == 8 || fs_khz == 12 {
            st.predict_lpc_order = MIN_LPC_ORDER;
            st.nlsf_cb = &NLSF_CB_NB_MB;
        } else {
            st.predict_lpc_order = MAX_LPC_ORDER;
            st.nlsf_cb = &NLSF_CB_WB;
        }
        let fs = fs_khz as usize;
        st.subfr_length = 5 * fs;
        st.frame_length = st.subfr_length * st.nb_subfr;
        st.ltp_mem_length = 20 * fs;
        st.la_pitch = 2 * fs;
        st.max_pitch_lag = 18 * fs;
        st.pitch_lpc_win_length = if st.nb_subfr == MAX_NB_SUBFR {
            FIND_PITCH_LPC_WIN_MS * fs
        } else {
            FIND_PITCH_LPC_WIN_MS_2_SF * fs
        };
        st.pitch_lag_low_bits_icdf = match fs_khz {
            16 => &UNIFORM8_ICDF,
            12 => &UNIFORM6_ICDF,
            _ => &UNIFORM4_ICDF,
        };
    }
}

/// `silk_setup_complexity`.
fn setup_complexity(st: &mut ChannelEncoder, complexity: i32) {
    let fs = st.fs_khz as usize;
    let warp = st.fs_khz * super::fix::fix_f32(WARPING_MULTIPLIER, 16);
    // (pitch complexity, threshold, pitch order, shaping order, la_shape ms, states,
    // interpolated NLSFs, survivors, warping)
    let (pc, thr, po, so, la, nst, interp, surv, w) = match complexity {
        0 => (SILK_PE_MIN_COMPLEX, 0.8, 6, 12, 3, 1, false, 2, 0),
        1 => (SILK_PE_MID_COMPLEX, 0.76, 8, 14, 5, 1, false, 3, 0),
        2 => (SILK_PE_MIN_COMPLEX, 0.8, 6, 12, 3, 2, false, 2, 0),
        3 => (SILK_PE_MID_COMPLEX, 0.76, 8, 14, 5, 2, false, 4, 0),
        4 | 5 => (SILK_PE_MID_COMPLEX, 0.74, 10, 16, 5, 2, true, 6, warp),
        6 | 7 => (SILK_PE_MID_COMPLEX, 0.72, 12, 20, 5, 3, true, 8, warp),
        _ => (SILK_PE_MAX_COMPLEX, 0.7, 16, 24, 5, 4, true, 16, warp),
    };
    st.pitch_estimation_complexity = pc;
    st.pitch_estimation_threshold_q16 = fix_f64(thr, 16);
    st.pitch_estimation_lpc_order = po;
    st.shaping_lpc_order = so;
    st.la_shape = la * fs;
    st.n_states_delayed_decision = nst;
    st.use_interpolated_nlsfs = interp;
    st.nlsf_msvq_survivors = surv;
    st.warping_q16 = w;
    st.pitch_estimation_lpc_order = st.pitch_estimation_lpc_order.min(st.predict_lpc_order);
    st.shape_win_length = 5 * fs + 2 * st.la_shape;
    st.complexity = complexity;
}

/// `silk_setup_LBRR`.
fn setup_lbrr(st: &mut ChannelEncoder, c: &EncControl) {
    let lbrr_in_previous_packet = st.lbrr_enabled;
    st.lbrr_enabled = c.lbrr_coded;
    if st.lbrr_enabled {
        st.lbrr_gain_increases = if !lbrr_in_previous_packet {
            7
        } else {
            (7 - smulwb(st.packet_loss_perc, fix_f64(0.2, 16))).max(3)
        };
    }
}

/// `silk_control_audio_bandwidth`: the internal rate in kHz, stepping down or up through
/// the low-pass transition when allowed.
fn control_audio_bandwidth(st: &mut ChannelEncoder, c: &mut EncControl) -> i32 {
    let mut orig_khz = st.fs_khz;
    if orig_khz == 0 {
        orig_khz = st.s_lp.saved_fs_khz;
    }
    let mut fs_khz = orig_khz;
    let mut fs_hz = fs_khz * 1000;
    if fs_hz == 0 {
        fs_hz = st.desired_internal_fs_hz.min(st.api_fs_hz);
        fs_khz = fs_hz / 1000;
    } else if fs_hz > st.api_fs_hz || fs_hz > st.max_internal_fs_hz || fs_hz < st.min_internal_fs_hz
    {
        fs_hz = st
            .api_fs_hz
            .min(st.max_internal_fs_hz)
            .max(st.min_internal_fs_hz);
        fs_khz = fs_hz / 1000;
    } else {
        if st.s_lp.transition_frame_no >= TRANSITION_FRAMES {
            st.s_lp.mode = 0;
        }
        if st.allow_bandwidth_switch || c.opus_can_switch {
            if orig_khz * 1000 > st.desired_internal_fs_hz {
                // Down.
                if st.s_lp.mode == 0 {
                    st.s_lp.transition_frame_no = TRANSITION_FRAMES;
                    st.s_lp.in_lp_state = [0; 2];
                }
                if c.opus_can_switch {
                    st.s_lp.mode = 0;
                    fs_khz = if orig_khz == 16 { 12 } else { 8 };
                } else if st.s_lp.transition_frame_no <= 0 {
                    c.switch_ready = true;
                    // Room for the redundant frame.
                    c.max_bits -= c.max_bits * 5 / (c.payload_size_ms + 5);
                } else {
                    // Down at double speed.
                    st.s_lp.mode = -2;
                }
            } else if orig_khz * 1000 < st.desired_internal_fs_hz {
                // Up.
                if c.opus_can_switch {
                    fs_khz = if orig_khz == 8 { 12 } else { 16 };
                    st.s_lp.transition_frame_no = 0;
                    st.s_lp.in_lp_state = [0; 2];
                    st.s_lp.mode = 1;
                } else if st.s_lp.mode == 0 {
                    c.switch_ready = true;
                    c.max_bits -= c.max_bits * 5 / (c.payload_size_ms + 5);
                } else {
                    st.s_lp.mode = 1;
                }
            } else if st.s_lp.mode < 0 {
                st.s_lp.mode = 1;
            }
        }
    }
    fs_khz
}

/// The variable low-pass state, kept across a bandwidth-switching reset (`prefillFlag == 2`).
pub(super) fn saved_lp(st: &ChannelEncoder) -> LpState {
    LpState {
        saved_fs_khz: st.fs_khz,
        ..st.s_lp
    }
}
