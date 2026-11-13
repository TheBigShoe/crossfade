//! The decoder's state for one coded channel (libopus `silk/structs.h`: `silk_decoder_state`,
//! `silk_decoder_control`, `SideInfoIndices`, the PLC and CNG structs) and `define.h`'s
//! constants.

use super::resampler::Resampler;
use super::tables::{PITCH_CONTOUR_NB_ICDF, UNIFORM4_ICDF};
use super::tables_nlsf::{NLSF_CB_NB_MB, NlsfCodebook};

pub(super) const MAX_FRAMES_PER_PACKET: usize = 3;
pub(super) const MAX_NB_SUBFR: usize = 4;
pub(super) const SUB_FRAME_LENGTH_MS: usize = 5;
pub(super) const LTP_MEM_LENGTH_MS: usize = 20;
pub(super) const MAX_FS_KHZ: usize = 16;
pub(super) const MAX_SUB_FRAME_LENGTH: usize = SUB_FRAME_LENGTH_MS * MAX_FS_KHZ;
pub(super) const MAX_FRAME_LENGTH: usize = MAX_SUB_FRAME_LENGTH * MAX_NB_SUBFR;
pub(super) const MAX_LPC_ORDER: usize = 16;
pub(super) const MIN_LPC_ORDER: usize = 10;
pub(super) const LTP_ORDER: usize = 5;
pub(super) const SHELL_CODEC_FRAME_LENGTH: usize = 16;

pub(super) const TYPE_NO_VOICE_ACTIVITY: i32 = 0;
pub(super) const TYPE_VOICED: i32 = 2;

/// How a frame's first gain and LTP scaling are coded (`CODE_INDEPENDENTLY`, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CondCoding {
    Independently,
    IndependentlyNoLtpScaling,
    Conditionally,
}

/// `lostFlag`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LostFlag {
    Normal,
    PacketLost,
    DecodeLbrr,
}

/// `SideInfoIndices`: a frame's decoded quantisation indices.
#[derive(Debug, Clone, Default)]
pub(super) struct SideInfoIndices {
    pub(super) gains_indices: [i8; MAX_NB_SUBFR],
    pub(super) ltp_index: [i8; MAX_NB_SUBFR],
    pub(super) nlsf_indices: [i8; MAX_LPC_ORDER + 1],
    pub(super) lag_index: i16,
    pub(super) contour_index: i8,
    pub(super) signal_type: i8,
    pub(super) quant_offset_type: i8,
    pub(super) nlsf_interp_coef_q2: i8,
    pub(super) per_index: i8,
    pub(super) ltp_scale_index: i8,
    pub(super) seed: i8,
}

/// `silk_PLC_struct`.
#[derive(Debug, Clone, Default)]
pub(super) struct PlcState {
    pub(super) pitch_l_q8: i32,
    pub(super) ltp_coef_q14: [i16; LTP_ORDER],
    pub(super) prev_lpc_q12: [i16; MAX_LPC_ORDER],
    pub(super) last_frame_lost: bool,
    pub(super) rand_seed: i32,
    pub(super) rand_scale_q14: i16,
    pub(super) conc_energy: i32,
    pub(super) conc_energy_shift: i32,
    pub(super) prev_ltp_scale_q14: i16,
    pub(super) prev_gain_q16: [i32; 2],
    pub(super) fs_khz: i32,
    pub(super) nb_subfr: usize,
    pub(super) subfr_length: usize,
}

/// `silk_CNG_struct`.
#[derive(Debug, Clone)]
pub(super) struct CngState {
    pub(super) exc_buf_q14: [i32; MAX_FRAME_LENGTH],
    pub(super) smth_nlsf_q15: [i16; MAX_LPC_ORDER],
    pub(super) synth_state: [i32; MAX_LPC_ORDER],
    pub(super) smth_gain_q16: i32,
    pub(super) rand_seed: i32,
    pub(super) fs_khz: i32,
}

impl Default for CngState {
    fn default() -> Self {
        Self {
            exc_buf_q14: [0; MAX_FRAME_LENGTH],
            smth_nlsf_q15: [0; MAX_LPC_ORDER],
            synth_state: [0; MAX_LPC_ORDER],
            smth_gain_q16: 0,
            rand_seed: 0,
            fs_khz: 0,
        }
    }
}

/// `silk_decoder_control`: one frame's dequantised parameters.
#[derive(Debug, Clone, Default)]
pub(super) struct DecoderControl {
    pub(super) pitch_l: [i32; MAX_NB_SUBFR],
    pub(super) gains_q16: [i32; MAX_NB_SUBFR],
    pub(super) pred_coef_q12: [[i16; MAX_LPC_ORDER]; 2],
    pub(super) ltp_coef_q14: [i16; LTP_ORDER * MAX_NB_SUBFR],
    pub(super) ltp_scale_q14: i32,
}

/// `silk_decoder_state`: one coded channel (mid or side).
#[derive(Debug, Clone)]
pub(super) struct ChannelState {
    pub(super) prev_gain_q16: i32,
    pub(super) exc_q14: [i32; MAX_FRAME_LENGTH],
    pub(super) slpc_q14_buf: [i32; MAX_LPC_ORDER],
    pub(super) out_buf: [i16; MAX_FRAME_LENGTH + 2 * MAX_SUB_FRAME_LENGTH],
    pub(super) lag_prev: i32,
    pub(super) last_gain_index: i8,
    pub(super) fs_khz: i32,
    pub(super) fs_api_hz: i32,
    pub(super) nb_subfr: usize,
    pub(super) frame_length: usize,
    pub(super) subfr_length: usize,
    pub(super) ltp_mem_length: usize,
    pub(super) lpc_order: usize,
    pub(super) prev_nlsf_q15: [i16; MAX_LPC_ORDER],
    pub(super) first_frame_after_reset: bool,
    pub(super) pitch_lag_low_bits_icdf: &'static [u8],
    pub(super) pitch_contour_icdf: &'static [u8],
    pub(super) n_frames_decoded: usize,
    pub(super) n_frames_per_packet: usize,
    pub(super) ec_prev_signal_type: i32,
    pub(super) ec_prev_lag_index: i16,
    pub(super) vad_flags: [bool; MAX_FRAMES_PER_PACKET],
    pub(super) lbrr_flag: bool,
    pub(super) lbrr_flags: [bool; MAX_FRAMES_PER_PACKET],
    pub(super) resampler: Resampler,
    pub(super) nlsf_cb: &'static NlsfCodebook,
    pub(super) indices: SideInfoIndices,
    pub(super) cng: CngState,
    pub(super) loss_cnt: i32,
    pub(super) prev_signal_type: i32,
    pub(super) plc: PlcState,
}

impl ChannelState {
    /// `silk_init_decoder`.
    pub(super) fn new() -> Self {
        let mut st = Self {
            prev_gain_q16: 0,
            exc_q14: [0; MAX_FRAME_LENGTH],
            slpc_q14_buf: [0; MAX_LPC_ORDER],
            out_buf: [0; MAX_FRAME_LENGTH + 2 * MAX_SUB_FRAME_LENGTH],
            lag_prev: 0,
            last_gain_index: 0,
            fs_khz: 0,
            fs_api_hz: 0,
            nb_subfr: 0,
            frame_length: 0,
            subfr_length: 0,
            ltp_mem_length: 0,
            lpc_order: 0,
            prev_nlsf_q15: [0; MAX_LPC_ORDER],
            first_frame_after_reset: false,
            // The C's pointers are null until `silk_decoder_set_fs`, which always runs first.
            pitch_lag_low_bits_icdf: &UNIFORM4_ICDF,
            pitch_contour_icdf: &PITCH_CONTOUR_NB_ICDF,
            n_frames_decoded: 0,
            n_frames_per_packet: 0,
            ec_prev_signal_type: 0,
            ec_prev_lag_index: 0,
            vad_flags: [false; MAX_FRAMES_PER_PACKET],
            lbrr_flag: false,
            lbrr_flags: [false; MAX_FRAMES_PER_PACKET],
            resampler: Resampler::default(),
            nlsf_cb: &NLSF_CB_NB_MB,
            indices: SideInfoIndices::default(),
            cng: CngState::default(),
            loss_cnt: 0,
            prev_signal_type: 0,
            plc: PlcState::default(),
        };
        st.first_frame_after_reset = true;
        st.prev_gain_q16 = 65536;
        super::cng::cng_reset(&mut st);
        super::plc::plc_reset(&mut st);
        st
    }
}
