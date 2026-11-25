//! The encoder's state for one coded channel (libopus `silk/structs.h`'s `silk_nsq_state`,
//! `silk_VAD_state`, `silk_LP_state`, `stereo_enc_state`, `silk_encoder_state`;
//! `silk/fixed/structs_FIX.h`'s shape state, `silk_encoder_state_FIX` and
//! `silk_encoder_control_FIX`), merged into one struct per channel as the fixed-point build
//! has them.

use super::sigproc::MAX_SHAPE_LPC_ORDER;
use crate::silk::state::{
    LTP_ORDER, MAX_FRAME_LENGTH, MAX_FRAMES_PER_PACKET, MAX_LPC_ORDER, MAX_NB_SUBFR,
    MAX_SUB_FRAME_LENGTH, SideInfoIndices,
};
use crate::silk::tables::{PITCH_CONTOUR_NB_ICDF, UNIFORM4_ICDF};
use crate::silk::tables_nlsf::{NLSF_CB_NB_MB, NlsfCodebook};

pub(super) const VAD_N_BANDS: usize = 4;
pub(super) const NSQ_LPC_BUF_LENGTH: usize = MAX_LPC_ORDER;
pub(super) const LA_SHAPE_MS: usize = 5;
pub(super) const LA_SHAPE_MAX: usize = LA_SHAPE_MS * 16;
pub(super) const TYPE_UNVOICED: i32 = 1;

/// `silk_nsq_state`: what the noise-shaping quantiser carries between frames, mirroring the
/// decoder's synthesis state.
#[derive(Debug, Clone)]
pub(super) struct NsqState {
    pub(super) xq: [i16; 2 * MAX_FRAME_LENGTH],
    pub(super) s_ltp_shp_q14: [i32; 2 * MAX_FRAME_LENGTH],
    pub(super) s_lpc_q14: [i32; MAX_SUB_FRAME_LENGTH + NSQ_LPC_BUF_LENGTH],
    pub(super) s_ar2_q14: [i32; MAX_SHAPE_LPC_ORDER],
    pub(super) s_lf_ar_shp_q14: i32,
    pub(super) s_diff_shp_q14: i32,
    pub(super) lag_prev: i32,
    pub(super) s_ltp_buf_idx: usize,
    pub(super) s_ltp_shp_buf_idx: usize,
    pub(super) rand_seed: i32,
    pub(super) prev_gain_q16: i32,
    pub(super) rewhite_flag: bool,
}

impl Default for NsqState {
    fn default() -> Self {
        Self {
            xq: [0; 2 * MAX_FRAME_LENGTH],
            s_ltp_shp_q14: [0; 2 * MAX_FRAME_LENGTH],
            s_lpc_q14: [0; MAX_SUB_FRAME_LENGTH + NSQ_LPC_BUF_LENGTH],
            s_ar2_q14: [0; MAX_SHAPE_LPC_ORDER],
            s_lf_ar_shp_q14: 0,
            s_diff_shp_q14: 0,
            lag_prev: 0,
            s_ltp_buf_idx: 0,
            s_ltp_shp_buf_idx: 0,
            rand_seed: 0,
            prev_gain_q16: 0,
            rewhite_flag: false,
        }
    }
}

/// `silk_VAD_state`.
#[derive(Debug, Clone, Default)]
pub(super) struct VadState {
    pub(super) ana_state: [i32; 2],
    pub(super) ana_state1: [i32; 2],
    pub(super) ana_state2: [i32; 2],
    pub(super) xnrg_subfr: [i32; VAD_N_BANDS],
    pub(super) nrg_ratio_smth_q8: [i32; VAD_N_BANDS],
    pub(super) hp_state: i16,
    pub(super) nl: [i32; VAD_N_BANDS],
    pub(super) inv_nl: [i32; VAD_N_BANDS],
    pub(super) noise_level_bias: [i32; VAD_N_BANDS],
    pub(super) counter: i32,
}

/// `silk_LP_state`: the variable low-pass filter of a bandwidth switch.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct LpState {
    pub(super) in_lp_state: [i32; 2],
    pub(super) transition_frame_no: i32,
    /// Below 0: switching down; above: switching up; 0: off.
    pub(super) mode: i32,
    /// The rate before a bandwidth switching reset, if non-zero.
    pub(super) saved_fs_khz: i32,
}

/// `silk_shape_state_FIX`.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ShapeState {
    pub(super) last_gain_index: i8,
    pub(super) harm_shape_gain_smth_q16: i32,
    pub(super) tilt_smth_q16: i32,
}

/// `silk_encoder_control_FIX`: one frame's parameters, from analysis to quantisation.
#[derive(Debug, Clone)]
pub(super) struct EncoderControl {
    pub(super) gains_q16: [i32; MAX_NB_SUBFR],
    pub(super) pred_coef_q12: [[i16; MAX_LPC_ORDER]; 2],
    pub(super) ltp_coef_q14: [i16; LTP_ORDER * MAX_NB_SUBFR],
    pub(super) ltp_scale_q14: i32,
    pub(super) pitch_l: [i32; MAX_NB_SUBFR],
    pub(super) ar_q13: [i16; MAX_NB_SUBFR * MAX_SHAPE_LPC_ORDER],
    /// Two 16-bit coefficients per subframe, packed.
    pub(super) lf_shp_q14: [i32; MAX_NB_SUBFR],
    pub(super) tilt_q14: [i32; MAX_NB_SUBFR],
    pub(super) harm_shape_gain_q14: [i32; MAX_NB_SUBFR],
    pub(super) lambda_q10: i32,
    pub(super) input_quality_q14: i32,
    pub(super) coding_quality_q14: i32,
    pub(super) pred_gain_q16: i32,
    pub(super) lt_pred_cod_gain_q7: i32,
    pub(super) res_nrg: [i32; MAX_NB_SUBFR],
    pub(super) res_nrg_q: [i32; MAX_NB_SUBFR],
    pub(super) gains_unq_q16: [i32; MAX_NB_SUBFR],
    pub(super) last_gain_index_prev: i8,
}

impl Default for EncoderControl {
    fn default() -> Self {
        Self {
            gains_q16: [0; MAX_NB_SUBFR],
            pred_coef_q12: [[0; MAX_LPC_ORDER]; 2],
            ltp_coef_q14: [0; LTP_ORDER * MAX_NB_SUBFR],
            ltp_scale_q14: 0,
            pitch_l: [0; MAX_NB_SUBFR],
            ar_q13: [0; MAX_NB_SUBFR * MAX_SHAPE_LPC_ORDER],
            lf_shp_q14: [0; MAX_NB_SUBFR],
            tilt_q14: [0; MAX_NB_SUBFR],
            harm_shape_gain_q14: [0; MAX_NB_SUBFR],
            lambda_q10: 0,
            input_quality_q14: 0,
            coding_quality_q14: 0,
            pred_gain_q16: 0,
            lt_pred_cod_gain_q7: 0,
            res_nrg: [0; MAX_NB_SUBFR],
            res_nrg_q: [0; MAX_NB_SUBFR],
            gains_unq_q16: [0; MAX_NB_SUBFR],
            last_gain_index_prev: 0,
        }
    }
}

/// `silk_encoder_state_FIX` with its common `silk_encoder_state`: one coded channel.
#[derive(Debug, Clone)]
pub(super) struct ChannelEncoder {
    pub(super) in_hp_state: [i32; 2],
    pub(super) variable_hp_smth1_q15: i32,
    pub(super) variable_hp_smth2_q15: i32,
    pub(super) s_lp: LpState,
    pub(super) s_vad: VadState,
    pub(super) s_nsq: NsqState,
    pub(super) prev_nlsfq_q15: [i16; MAX_LPC_ORDER],
    pub(super) speech_activity_q8: i32,
    pub(super) allow_bandwidth_switch: bool,
    pub(super) lbrr_prev_last_gain_index: i8,
    pub(super) prev_signal_type: i32,
    pub(super) prev_lag: i32,
    pub(super) pitch_lpc_win_length: usize,
    pub(super) max_pitch_lag: usize,
    pub(super) api_fs_hz: i32,
    pub(super) prev_api_fs_hz: i32,
    pub(super) max_internal_fs_hz: i32,
    pub(super) min_internal_fs_hz: i32,
    pub(super) desired_internal_fs_hz: i32,
    pub(super) fs_khz: i32,
    pub(super) nb_subfr: usize,
    pub(super) frame_length: usize,
    pub(super) subfr_length: usize,
    pub(super) ltp_mem_length: usize,
    pub(super) la_pitch: usize,
    pub(super) la_shape: usize,
    pub(super) shape_win_length: usize,
    pub(super) target_rate_bps: i32,
    pub(super) packet_size_ms: i32,
    pub(super) packet_loss_perc: i32,
    pub(super) frame_counter: i32,
    pub(super) complexity: i32,
    pub(super) n_states_delayed_decision: usize,
    pub(super) use_interpolated_nlsfs: bool,
    pub(super) shaping_lpc_order: usize,
    pub(super) predict_lpc_order: usize,
    pub(super) pitch_estimation_complexity: usize,
    pub(super) pitch_estimation_lpc_order: usize,
    pub(super) pitch_estimation_threshold_q16: i32,
    pub(super) sum_log_gain_q7: i32,
    pub(super) nlsf_msvq_survivors: usize,
    pub(super) first_frame_after_reset: bool,
    pub(super) controlled_since_last_payload: bool,
    pub(super) warping_q16: i32,
    pub(super) use_cbr: bool,
    pub(super) prefill_flag: bool,
    pub(super) pitch_lag_low_bits_icdf: &'static [u8],
    pub(super) pitch_contour_icdf: &'static [u8],
    pub(super) nlsf_cb: &'static NlsfCodebook,
    pub(super) input_quality_bands_q15: [i32; VAD_N_BANDS],
    pub(super) input_tilt_q15: i32,
    pub(super) snr_db_q7: i32,
    pub(super) vad_flags: [bool; MAX_FRAMES_PER_PACKET],
    pub(super) lbrr_flag: bool,
    pub(super) lbrr_flags: [bool; MAX_FRAMES_PER_PACKET],
    pub(super) indices: SideInfoIndices,
    pub(super) pulses: [i8; MAX_FRAME_LENGTH],
    pub(super) input_buf: [i16; MAX_FRAME_LENGTH + 2],
    pub(super) input_buf_ix: usize,
    pub(super) n_frames_per_packet: usize,
    pub(super) n_frames_encoded: usize,
    pub(super) n_channels_api: usize,
    pub(super) n_channels_internal: usize,
    pub(super) channel_nb: usize,
    pub(super) ec_prev_signal_type: i32,
    pub(super) ec_prev_lag_index: i16,
    pub(super) resampler: super::resampler_enc::DownResampler,
    pub(super) use_dtx: bool,
    pub(super) in_dtx: bool,
    pub(super) no_speech_counter: i32,
    pub(super) lbrr_enabled: bool,
    pub(super) lbrr_gain_increases: i32,
    pub(super) indices_lbrr: [SideInfoIndices; MAX_FRAMES_PER_PACKET],
    pub(super) pulses_lbrr: [[i8; MAX_FRAME_LENGTH]; MAX_FRAMES_PER_PACKET],
    // silk_encoder_state_FIX's own fields.
    pub(super) s_shape: ShapeState,
    pub(super) x_buf: [i16; 2 * MAX_FRAME_LENGTH + LA_SHAPE_MAX],
    pub(super) ltp_corr_q15: i32,
}

impl ChannelEncoder {
    /// `silk_init_encoder`: the zeroed state with its few non-zero values.
    pub(super) fn new() -> Self {
        let mut st = Self {
            in_hp_state: [0; 2],
            variable_hp_smth1_q15: 0,
            variable_hp_smth2_q15: 0,
            s_lp: LpState::default(),
            s_vad: VadState::default(),
            s_nsq: NsqState::default(),
            prev_nlsfq_q15: [0; MAX_LPC_ORDER],
            speech_activity_q8: 0,
            allow_bandwidth_switch: false,
            lbrr_prev_last_gain_index: 0,
            prev_signal_type: 0,
            prev_lag: 0,
            pitch_lpc_win_length: 0,
            max_pitch_lag: 0,
            api_fs_hz: 0,
            prev_api_fs_hz: 0,
            max_internal_fs_hz: 0,
            min_internal_fs_hz: 0,
            desired_internal_fs_hz: 0,
            fs_khz: 0,
            nb_subfr: 0,
            frame_length: 0,
            subfr_length: 0,
            ltp_mem_length: 0,
            la_pitch: 0,
            la_shape: 0,
            shape_win_length: 0,
            target_rate_bps: 0,
            packet_size_ms: 0,
            packet_loss_perc: 0,
            frame_counter: 0,
            complexity: 0,
            n_states_delayed_decision: 0,
            use_interpolated_nlsfs: false,
            shaping_lpc_order: 0,
            predict_lpc_order: 0,
            pitch_estimation_complexity: 0,
            pitch_estimation_lpc_order: 0,
            pitch_estimation_threshold_q16: 0,
            sum_log_gain_q7: 0,
            nlsf_msvq_survivors: 0,
            first_frame_after_reset: true,
            controlled_since_last_payload: false,
            warping_q16: 0,
            use_cbr: false,
            prefill_flag: false,
            // The C's pointers are null until `silk_setup_fs`, which always runs first.
            pitch_lag_low_bits_icdf: &UNIFORM4_ICDF,
            pitch_contour_icdf: &PITCH_CONTOUR_NB_ICDF,
            nlsf_cb: &NLSF_CB_NB_MB,
            input_quality_bands_q15: [0; VAD_N_BANDS],
            input_tilt_q15: 0,
            snr_db_q7: 0,
            vad_flags: [false; MAX_FRAMES_PER_PACKET],
            lbrr_flag: false,
            lbrr_flags: [false; MAX_FRAMES_PER_PACKET],
            indices: SideInfoIndices::default(),
            pulses: [0; MAX_FRAME_LENGTH],
            input_buf: [0; MAX_FRAME_LENGTH + 2],
            input_buf_ix: 0,
            n_frames_per_packet: 0,
            n_frames_encoded: 0,
            n_channels_api: 0,
            n_channels_internal: 0,
            channel_nb: 0,
            ec_prev_signal_type: 0,
            ec_prev_lag_index: 0,
            resampler: super::resampler_enc::DownResampler::default(),
            use_dtx: false,
            in_dtx: false,
            no_speech_counter: 0,
            lbrr_enabled: false,
            lbrr_gain_increases: 0,
            indices_lbrr: Default::default(),
            pulses_lbrr: [[0; MAX_FRAME_LENGTH]; MAX_FRAMES_PER_PACKET],
            s_shape: ShapeState::default(),
            x_buf: [0; 2 * MAX_FRAME_LENGTH + LA_SHAPE_MAX],
            ltp_corr_q15: 0,
        };
        let min_cutoff_log = super::fix::lin2log(super::fix::fix_f64(60.0, 16)) - (16 << 7);
        st.variable_hp_smth1_q15 = super::fix::lshift(min_cutoff_log, 8);
        st.variable_hp_smth2_q15 = st.variable_hp_smth1_q15;
        super::vad::vad_init(&mut st.s_vad);
        st
    }
}
