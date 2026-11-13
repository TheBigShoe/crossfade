//! One SILK frame through the decoder (libopus `silk/dec_API.c`'s `silk_Decode`,
//! `decode_frame.c`, `decoder_set_fs.c`): the packet header on a packet's first frame (VAD and
//! LBRR flags, the LBRR frames read past or decoded), the stereo predictor, each coded channel,
//! mid/side to left/right, and the resampler to 48 kHz.

use super::cng::cng;
use super::decode_core::decode_core;
use super::decode_indices::decode_indices;
use super::decode_parameters::decode_parameters;
use super::decode_pulses::decode_pulses;
use super::plc::{glue_frames, plc};
use super::resampler::Resampler;
use super::state::{
    ChannelState, CondCoding, DecoderControl, LTP_MEM_LENGTH_MS, LostFlag, MAX_FRAME_LENGTH,
    MAX_FRAMES_PER_PACKET, MAX_LPC_ORDER, MAX_NB_SUBFR, MIN_LPC_ORDER, SUB_FRAME_LENGTH_MS,
    TYPE_NO_VOICE_ACTIVITY,
};
use super::stereo::{StereoState, decode_mid_only, decode_pred, ms_to_lr};
use super::tables::{
    LBRR_FLAGS_ICDF, PITCH_CONTOUR_10_MS_ICDF, PITCH_CONTOUR_10_MS_NB_ICDF, PITCH_CONTOUR_ICDF,
    PITCH_CONTOUR_NB_ICDF, UNIFORM4_ICDF, UNIFORM6_ICDF, UNIFORM8_ICDF,
};
use super::tables_nlsf::{NLSF_CB_NB_MB, NLSF_CB_WB};
use crate::error::{Error, Result};
use crate::range::RangeDecoder;

/// 20 ms at 48 kHz: the most one SILK frame resamples to.
pub(super) const MAX_API_FRAME: usize = 960;
const API_RATE: i32 = 48_000;

/// `silk_DecControlStruct`, the fields the decoder reads.
#[derive(Debug, Clone, Copy)]
pub(super) struct DecControl {
    pub(super) n_channels_api: usize,
    pub(super) n_channels_internal: usize,
    pub(super) internal_sample_rate: i32,
    pub(super) payload_size_ms: usize,
}

/// The `silk_decoder` super struct.
#[derive(Debug, Clone)]
pub(super) struct SilkState {
    pub(super) channels: [ChannelState; 2],
    pub(super) stereo: StereoState,
    pub(super) n_channels_api: usize,
    pub(super) n_channels_internal: usize,
    pub(super) prev_decode_only_middle: bool,
}

impl SilkState {
    pub(super) fn new() -> Self {
        Self {
            channels: [ChannelState::new(), ChannelState::new()],
            stereo: StereoState::default(),
            n_channels_api: 0,
            n_channels_internal: 0,
            prev_decode_only_middle: false,
        }
    }

    /// `silk_ResetDecoder`, with RFC 8251 section 3's stereo reset. The channel counts of the
    /// last packet survive, as in libopus.
    pub(super) fn reset(&mut self) {
        self.channels = [ChannelState::new(), ChannelState::new()];
        self.stereo = StereoState::default();
        self.prev_decode_only_middle = false;
    }

    /// `silk_Decode`: one SILK frame (10 or 20 ms) into `out` (interleaved, `n_channels_api`
    /// channels, 48 kHz); returns the samples per channel written.
    pub(super) fn decode(
        &mut self,
        ctl: &DecControl,
        lost: LostFlag,
        new_packet: bool,
        mut dec: Option<&mut RangeDecoder<'_>>,
        out: &mut [i16],
    ) -> Result<usize> {
        let n_int = ctl.n_channels_internal;
        if !(1..=2).contains(&n_int) || !(1..=2).contains(&ctl.n_channels_api) {
            return Err(Error::Corrupt("silk: bad channel count"));
        }
        let mut decode_only_middle = false;
        let mut ms_pred_q13 = [0i32; 2];

        if new_packet {
            for ch in &mut self.channels[..n_int] {
                ch.n_frames_decoded = 0;
            }
        }
        // Mono to stereo in the bitstream: start the side channel afresh.
        if n_int > self.n_channels_internal {
            self.channels[1] = ChannelState::new();
        }
        let stereo_to_mono = n_int == 1
            && self.n_channels_internal == 2
            && ctl.internal_sample_rate == 1000 * self.channels[0].fs_khz;

        if self.channels[0].n_frames_decoded == 0 {
            let (frames, nb_subfr) = match ctl.payload_size_ms {
                0 | 10 => (1, 2),
                20 => (1, 4),
                40 => (2, 4),
                60 => (3, 4),
                _ => return Err(Error::Corrupt("silk: bad frame size")),
            };
            let fs_khz = (ctl.internal_sample_rate >> 10) + 1;
            if ![8, 12, 16].contains(&fs_khz) {
                return Err(Error::Corrupt("silk: bad internal rate"));
            }
            for ch in &mut self.channels[..n_int] {
                ch.n_frames_per_packet = frames;
                ch.nb_subfr = nb_subfr;
                decoder_set_fs(ch, fs_khz, API_RATE);
            }
        }

        if ctl.n_channels_api == 2
            && n_int == 2
            && (self.n_channels_api == 1 || self.n_channels_internal == 1)
        {
            self.stereo.pred_prev_q13 = [0; 2];
            self.stereo.s_side = [0; 2];
            self.channels[1].resampler = self.channels[0].resampler.clone();
        }
        self.n_channels_api = ctl.n_channels_api;
        self.n_channels_internal = n_int;

        if lost != LostFlag::PacketLost && self.channels[0].n_frames_decoded == 0 {
            let Some(dec) = dec.as_deref_mut() else {
                return Err(Error::Corrupt("silk: no data"));
            };
            self.decode_header(dec, n_int, lost)?;
        }

        let f0 = self.channels[0].n_frames_decoded;
        if f0 >= MAX_FRAMES_PER_PACKET {
            return Err(Error::Corrupt("silk: too many frames"));
        }
        if n_int == 2 {
            let coded = lost == LostFlag::Normal
                || (lost == LostFlag::DecodeLbrr && self.channels[0].lbrr_flags[f0]);
            if coded {
                let Some(dec) = dec.as_deref_mut() else {
                    return Err(Error::Corrupt("silk: no data"));
                };
                ms_pred_q13 = decode_pred(dec);
                let side_skipped = match lost {
                    LostFlag::Normal => !self.channels[1].vad_flags[f0],
                    _ => !self.channels[1].lbrr_flags[f0],
                };
                decode_only_middle = side_skipped && decode_mid_only(dec);
            } else {
                ms_pred_q13 = self.stereo.pred_prev_q13.map(i32::from);
            }
        }

        // The first frame with a side channel after mid-only ones starts its memory afresh.
        if n_int == 2 && !decode_only_middle && self.prev_decode_only_middle {
            let side = &mut self.channels[1];
            side.out_buf.fill(0);
            side.slpc_q14_buf = [0; MAX_LPC_ORDER];
            side.lag_prev = 100;
            side.last_gain_index = 10;
            side.prev_signal_type = TYPE_NO_VOICE_ACTIVITY;
            side.first_frame_after_reset = true;
        }

        let has_side = if lost == LostFlag::Normal {
            !decode_only_middle
        } else {
            let f1 = self.channels[1]
                .n_frames_decoded
                .min(MAX_FRAMES_PER_PACKET - 1);
            !self.prev_decode_only_middle
                || (n_int == 2 && lost == LostFlag::DecodeLbrr && self.channels[1].lbrr_flags[f1])
        };

        // Each channel's frame at offset 2: room for the stereo delay line.
        let mut tmp = [[0i16; MAX_FRAME_LENGTH + 2]; 2];
        let mut n_out_dec = 0;
        for n in 0..n_int {
            if n == 0 || has_side {
                // Channel 0's count has already moved on when the side channel decodes.
                let frame_index = self.channels[0].n_frames_decoded as isize - n as isize;
                let cond = if frame_index <= 0 {
                    CondCoding::Independently
                } else if lost == LostFlag::DecodeLbrr {
                    if self.channels[n].lbrr_flags[frame_index as usize - 1] {
                        CondCoding::Conditionally
                    } else {
                        CondCoding::Independently
                    }
                } else if n > 0 && self.prev_decode_only_middle {
                    // A side frame skipped in this packet leaves the LTP state well defined.
                    CondCoding::IndependentlyNoLtpScaling
                } else {
                    CondCoding::Conditionally
                };
                n_out_dec = decode_frame(
                    &mut self.channels[n],
                    dec.as_deref_mut(),
                    &mut tmp[n][2..],
                    lost,
                    cond,
                );
            } else {
                tmp[n][2..2 + n_out_dec].fill(0);
            }
            self.channels[n].n_frames_decoded += 1;
        }

        let [mid, side] = &mut tmp;
        let fs_khz = self.channels[0].fs_khz as usize;
        if ctl.n_channels_api == 2 && n_int == 2 {
            ms_to_lr(&mut self.stereo, mid, side, ms_pred_q13, fs_khz, n_out_dec);
        } else {
            mid[..2].copy_from_slice(&self.stereo.s_mid);
            self.stereo
                .s_mid
                .copy_from_slice(&mid[n_out_dec..n_out_dec + 2]);
        }

        let n_out = n_out_dec * API_RATE as usize / (fs_khz * 1000);
        if n_out > MAX_API_FRAME || out.len() < n_out * ctl.n_channels_api {
            return Err(Error::Corrupt("silk: output too long"));
        }
        let api = ctl.n_channels_api;
        let mut resampled = [0i16; MAX_API_FRAME];
        for n in 0..api.min(n_int) {
            self.channels[n]
                .resampler
                .process(&mut resampled[..n_out], &tmp[n][1..1 + n_out_dec]);
            for (o, &s) in out[n..].iter_mut().step_by(api).zip(&resampled[..n_out]) {
                *o = s;
            }
        }
        if api == 2 && n_int == 1 {
            if stereo_to_mono {
                // The right channel's resampler may still hold the old stereo signal.
                self.channels[1]
                    .resampler
                    .process(&mut resampled[..n_out], &tmp[0][1..1 + n_out_dec]);
                for (o, &s) in out[1..].iter_mut().step_by(2).zip(&resampled[..n_out]) {
                    *o = s;
                }
            } else {
                for i in 0..n_out {
                    out[2 * i + 1] = out[2 * i];
                }
            }
        }

        if lost == LostFlag::PacketLost {
            // Lift the gain clamp so the energy does not bounce back after a loss.
            for ch in &mut self.channels[..self.n_channels_internal] {
                ch.last_gain_index = 10;
            }
        } else {
            self.prev_decode_only_middle = decode_only_middle;
        }
        Ok(n_out)
    }

    /// The VAD and LBRR flags of every frame, then the LBRR frames: read and dropped in normal
    /// decoding (their indices and pulses only advance the range decoder).
    fn decode_header(
        &mut self,
        dec: &mut RangeDecoder<'_>,
        n_int: usize,
        lost: LostFlag,
    ) -> Result<()> {
        for ch in &mut self.channels[..n_int] {
            for i in 0..ch.n_frames_per_packet {
                ch.vad_flags[i] = dec.dec_bit_logp(1);
            }
            ch.lbrr_flag = dec.dec_bit_logp(1);
        }
        for ch in &mut self.channels[..n_int] {
            ch.lbrr_flags = [false; MAX_FRAMES_PER_PACKET];
            if ch.lbrr_flag {
                if ch.n_frames_per_packet == 1 {
                    ch.lbrr_flags[0] = true;
                } else {
                    let table = LBRR_FLAGS_ICDF[ch.n_frames_per_packet - 2];
                    let symbol = dec.dec_icdf(table, 8) + 1;
                    for i in 0..ch.n_frames_per_packet {
                        ch.lbrr_flags[i] = (symbol >> i) & 1 != 0;
                    }
                }
            }
        }
        if lost == LostFlag::Normal {
            let mut pulses = [0i16; MAX_FRAME_LENGTH];
            for i in 0..self.channels[0].n_frames_per_packet {
                for n in 0..n_int {
                    if !self.channels[n].lbrr_flags[i] {
                        continue;
                    }
                    if n_int == 2 && n == 0 {
                        decode_pred(dec);
                        if !self.channels[1].lbrr_flags[i] {
                            decode_mid_only(dec);
                        }
                    }
                    let cond = if i > 0 && self.channels[n].lbrr_flags[i - 1] {
                        CondCoding::Conditionally
                    } else {
                        CondCoding::Independently
                    };
                    let ch = &mut self.channels[n];
                    decode_indices(ch, dec, i, true, cond);
                    decode_pulses(
                        dec,
                        &mut pulses,
                        ch.indices.signal_type.into(),
                        ch.indices.quant_offset_type.into(),
                        ch.frame_length,
                    );
                }
            }
        }
        Ok(())
    }
}

/// `silk_decoder_set_fs`: frame and subframe lengths for `fs_khz`, and on a rate change the
/// codebooks, the resampler and a fresh signal memory.
pub(super) fn decoder_set_fs(st: &mut ChannelState, fs_khz: i32, fs_api_hz: i32) {
    st.subfr_length = SUB_FRAME_LENGTH_MS * fs_khz as usize;
    let frame_length = st.nb_subfr * st.subfr_length;
    if st.fs_khz != fs_khz || st.fs_api_hz != fs_api_hz {
        st.resampler = Resampler::new(fs_khz * 1000);
        st.fs_api_hz = fs_api_hz;
    }
    if st.fs_khz != fs_khz || frame_length != st.frame_length {
        st.pitch_contour_icdf = match (fs_khz == 8, st.nb_subfr == MAX_NB_SUBFR) {
            (true, true) => &PITCH_CONTOUR_NB_ICDF,
            (true, false) => &PITCH_CONTOUR_10_MS_NB_ICDF,
            (false, true) => &PITCH_CONTOUR_ICDF,
            (false, false) => &PITCH_CONTOUR_10_MS_ICDF,
        };
        if st.fs_khz != fs_khz {
            st.ltp_mem_length = LTP_MEM_LENGTH_MS * fs_khz as usize;
            if fs_khz == 16 {
                st.lpc_order = MAX_LPC_ORDER;
                st.nlsf_cb = &NLSF_CB_WB;
                st.pitch_lag_low_bits_icdf = &UNIFORM8_ICDF;
            } else {
                st.lpc_order = MIN_LPC_ORDER;
                st.nlsf_cb = &NLSF_CB_NB_MB;
                st.pitch_lag_low_bits_icdf = if fs_khz == 12 {
                    &UNIFORM6_ICDF
                } else {
                    &UNIFORM4_ICDF
                };
            }
            st.first_frame_after_reset = true;
            st.lag_prev = 100;
            st.last_gain_index = 10;
            st.prev_signal_type = TYPE_NO_VOICE_ACTIVITY;
            st.out_buf.fill(0);
            st.slpc_q14_buf = [0; MAX_LPC_ORDER];
        }
        st.fs_khz = fs_khz;
        st.frame_length = frame_length;
    }
}

/// `silk_decode_frame`: one channel's frame (decoded, or concealed) into `out`; returns its
/// length.
fn decode_frame(
    st: &mut ChannelState,
    dec: Option<&mut RangeDecoder<'_>>,
    out: &mut [i16],
    lost: LostFlag,
    cond: CondCoding,
) -> usize {
    let l = st.frame_length;
    let mut ctrl = DecoderControl::default();
    let f = st.n_frames_decoded.min(MAX_FRAMES_PER_PACKET - 1);
    let coded = lost == LostFlag::Normal || (lost == LostFlag::DecodeLbrr && st.lbrr_flags[f]);
    let out = &mut out[..l];
    match dec {
        Some(dec) if coded => {
            let mut pulses = [0i16; MAX_FRAME_LENGTH];
            decode_indices(st, dec, f, lost == LostFlag::DecodeLbrr, cond);
            decode_pulses(
                dec,
                &mut pulses,
                st.indices.signal_type.into(),
                st.indices.quant_offset_type.into(),
                l,
            );
            decode_parameters(st, &mut ctrl, cond);
            decode_core(st, &mut ctrl, out, &pulses);
            update_out_buf(st, out);
            plc(st, &mut ctrl, out, false);
            st.loss_cnt = 0;
            st.prev_signal_type = st.indices.signal_type.into();
            st.first_frame_after_reset = false;
        }
        _ => {
            plc(st, &mut ctrl, out, true);
            update_out_buf(st, out);
        }
    }
    cng(st, &ctrl, out);
    glue_frames(st, out);
    st.lag_prev = ctrl.pitch_l[st.nb_subfr.min(MAX_NB_SUBFR) - 1];
    l
}

/// Shifts the output history and appends the frame.
fn update_out_buf(st: &mut ChannelState, frame: &[i16]) {
    let mv = st.ltp_mem_length - st.frame_length;
    st.out_buf
        .copy_within(st.frame_length..st.frame_length + mv, 0);
    st.out_buf[mv..mv + st.frame_length].copy_from_slice(frame);
}
