//! The SILK encoder: a port of libopus's fixed-point SILK encoder (`silk/enc_API.c` and
//! everything under it, `silk/fixed/` for the analysis), its modules named after the C files
//! they port. All integer arithmetic, as the decoder's, so the packets are the same bits on
//! every platform.
//!
//! # The contract with the Opus layer
//!
//! [`SilkEncoder`] is libopus's `silk_encoder`; [`SilkEncoder::new`] is `silk_InitEncoder`.
//! The Opus layer (a port of `opus_encoder.c`) owns one and calls [`SilkEncoder::encode`]
//! (`silk_Encode`) once per Opus frame with an [`EncControl`] (`silk_EncControlStruct`) it
//! keeps between calls:
//!
//! - **Input.** `pcm` holds one Opus frame (10, 20, 40 or 60 ms; `payload_size_ms` says
//!   which) of 16-bit samples at 48 kHz (`api_sample_rate` must be 48000), interleaved when
//!   `n_channels_api` is 2. SILK resamples to its internal rate itself (8, 12 or 16 kHz, the
//!   encoder-side `silk_resampler`, with its input delay). The Opus layer's high-pass or DC
//!   filter runs before, as in libopus (`hp_cutoff`/`dc_reject`, following
//!   [`SilkEncoder::variable_hp_smth1_q15`]).
//! - **Range encoder.** `enc` is the packet's [`RangeEncoder`], owned by the caller, just
//!   after the TOC byte; SILK codes first (its VAD and LBRR flags are patched in at the end
//!   with `patch_initial_bits`), and in hybrid mode CELT then codes on into the same encoder.
//!   A SILK-only packet ends with `(enc.tell() + 7) >> 3` bytes, as libopus's does.
//! - **Control in.** Channels (`n_channels_api`, and `n_channels_internal`: 1 codes a stereo
//!   input as mono), the internal rate range (`min_`, `max_` and
//!   `desired_internal_sample_rate`: 8000, 12000 or 16000), `payload_size_ms`, `bit_rate`
//!   (SILK's share, bits/s), `complexity` 0-10, `use_cbr`, `max_bits` (the budget for the whole
//!   packet: 8 times the bytes after the TOC), `packet_loss_percentage`, `lbrr_coded` (in-band
//!   FEC in this packet), `use_dtx`, `to_mono` (a smooth downmix before a switch to mono),
//!   `opus_can_switch` (the Opus layer can switch bandwidth now) and `reduced_dependency`.
//! - **Control out.** `internal_sample_rate` (for the TOC's bandwidth), `stereo_width_q14`,
//!   `allow_bandwidth_switch`, `in_wb_mode_without_variable_lp`, `switch_ready` (a bandwidth
//!   switch is due; `max_bits` was lowered to make room for a redundant CELT frame),
//!   `signal_type` and `offset` (the quantisation offset, which CELT's analysis reads); and
//!   [`SilkEncoder::encode`]'s value, the payload bytes (`nBytesOut`; 0 for a frame in DTX,
//!   which the Opus layer then sends as a TOC byte only).
//! - **Prefill.** [`Prefill::Reset`] (libopus's `prefillFlag` 1) and [`Prefill::KeepLowPass`]
//!   (2) take 10 ms to fill the buffers after a reset, coding nothing: `enc` is `None`.
//! - **Mid-only.** Whether a frame coded the mid channel only is [`SilkEncoder::mid_only`].
//!
//! Not ported: libopus's DRED and deep-learning features. LBRR and DTX follow `silk_Encode`;
//! the Opus layer decides when to ask for them.

mod analysis;
mod control;
mod encode_frame;
mod encode_indices;
mod encode_pulses;
mod fix;
mod lp_cutoff;
mod nlsf_enc;
mod noise_shape;
mod nsq;
mod nsq_del_dec;
mod pitch;
mod quant;
mod resampler_enc;
mod sigproc;
pub mod silk_only;
mod state;
mod stereo_enc;
mod tables;
mod vad;

use crate::error::{Error, Result};
use crate::range_enc::RangeEncoder;
use crate::silk::state::{CondCoding, MAX_FRAMES_PER_PACKET, TYPE_NO_VOICE_ACTIVITY, TYPE_VOICED};
use crate::silk::tables::{LBRR_FLAGS_ICDF, QUANTIZATION_OFFSETS_Q10};
use control::{check_control_input, control_encoder, control_snr, saved_lp};
use encode_frame::{encode_do_vad, encode_frame, encode_lbrr_frame};
use fix::{fix_f32, fix_f64, lin2log, smlawb, smulbb};
use state::{ChannelEncoder, NsqState, ShapeState};
use stereo_enc::{StereoEncState, encode_mid_only, encode_pred, lr_to_ms};

const BITRESERVOIR_DECAY_TIME_MS: i32 = 500;
const SPEECH_ACTIVITY_DTX_THRES: f32 = 0.05;
const MAX_BANDWIDTH_SWITCH_DELAY_MS: f32 = 5000.0;
const VARIABLE_HP_MIN_CUTOFF_HZ: i32 = 60;
const VARIABLE_HP_MAX_CUTOFF_HZ: i32 = 100;
const VARIABLE_HP_MAX_DELTA_FREQ: f32 = 0.4;
const VARIABLE_HP_SMTH_COEF1: f32 = 0.1;

/// `silk_EncControlStruct`: what the Opus layer asks of SILK, and what SILK reports back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EncControl {
    pub(crate) n_channels_api: usize,
    pub(crate) n_channels_internal: usize,
    pub(crate) api_sample_rate: i32,
    pub(crate) max_internal_sample_rate: i32,
    pub(crate) min_internal_sample_rate: i32,
    pub(crate) desired_internal_sample_rate: i32,
    pub(crate) payload_size_ms: i32,
    pub(crate) bit_rate: i32,
    pub(crate) packet_loss_percentage: i32,
    pub(crate) complexity: i32,
    pub(crate) lbrr_coded: bool,
    pub(crate) use_dtx: bool,
    pub(crate) use_cbr: bool,
    pub(crate) max_bits: i32,
    pub(crate) to_mono: bool,
    pub(crate) opus_can_switch: bool,
    pub(crate) reduced_dependency: bool,
    pub(crate) internal_sample_rate: i32,
    pub(crate) allow_bandwidth_switch: bool,
    pub(crate) in_wb_mode_without_variable_lp: bool,
    pub(crate) stereo_width_q14: i32,
    pub(crate) switch_ready: bool,
    pub(crate) signal_type: i32,
    pub(crate) offset: i32,
}

impl Default for EncControl {
    /// libopus's `opus_encoder_init` defaults: mono, 48 kHz, 8 to 16 kHz internally, 20 ms,
    /// 25 kb/s, complexity 9, VBR.
    fn default() -> Self {
        Self {
            n_channels_api: 1,
            n_channels_internal: 1,
            api_sample_rate: 48_000,
            max_internal_sample_rate: 16_000,
            min_internal_sample_rate: 8_000,
            desired_internal_sample_rate: 16_000,
            payload_size_ms: 20,
            bit_rate: 25_000,
            packet_loss_percentage: 0,
            complexity: 9,
            lbrr_coded: false,
            use_dtx: false,
            use_cbr: false,
            max_bits: 1275 * 8,
            to_mono: false,
            opus_can_switch: false,
            reduced_dependency: false,
            internal_sample_rate: 0,
            allow_bandwidth_switch: false,
            in_wb_mode_without_variable_lp: false,
            stereo_width_q14: 0,
            switch_ready: false,
            signal_type: 0,
            offset: 0,
        }
    }
}

/// `prefillFlag`: encode without coding, to fill the buffers after a reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Prefill {
    /// Code normally (0).
    No,
    /// Reset, then fill (1).
    Reset,
    /// Reset, keeping the bandwidth switch's low-pass state (2).
    KeepLowPass,
}

/// `silk_encoder`: the encoder's state for one stream.
#[derive(Debug, Clone)]
pub(crate) struct SilkEncoder {
    channels: Box<[ChannelEncoder; 2]>,
    stereo: StereoEncState,
    n_bits_used_lbrr: i32,
    n_bits_exceeded: i32,
    n_channels_api: usize,
    n_channels_internal: usize,
    n_prev_channels_internal: usize,
    time_since_switch_allowed_ms: i32,
    allow_bandwidth_switch: bool,
    prev_decode_only_middle: bool,
    /// One channel of one call's input.
    buf: Vec<i16>,
}

impl SilkEncoder {
    /// `silk_InitEncoder`.
    pub(crate) fn new() -> Self {
        Self {
            channels: Box::new([ChannelEncoder::new(), ChannelEncoder::new()]),
            stereo: StereoEncState::default(),
            n_bits_used_lbrr: 0,
            n_bits_exceeded: 0,
            n_channels_api: 1,
            n_channels_internal: 1,
            n_prev_channels_internal: 0,
            time_since_switch_allowed_ms: 0,
            allow_bandwidth_switch: false,
            prev_decode_only_middle: false,
            buf: vec![0; 60 * 48],
        }
    }

    /// The first channel's high-pass smoother (`variable_HP_smth1_Q15`), which the Opus
    /// layer's input filter follows.
    pub(crate) fn variable_hp_smth1_q15(&self) -> i32 {
        self.channels[0].variable_hp_smth1_q15
    }

    /// Whether frame `i` of the last packet coded the mid channel only.
    pub(crate) fn mid_only(&self, i: usize) -> bool {
        self.stereo.mid_only_flags[i.min(MAX_FRAMES_PER_PACKET - 1)]
    }

    /// `silk_Encode`: one Opus frame of `pcm` (see the module's contract); returns the payload
    /// bytes. `enc` is `None` only for a prefill.
    pub(crate) fn encode(
        &mut self,
        c: &mut EncControl,
        mut pcm: &[i16],
        mut enc: Option<&mut RangeEncoder>,
        prefill: Prefill,
        activity: Option<bool>,
    ) -> Result<usize> {
        if c.reduced_dependency {
            self.channels[0].first_frame_after_reset = true;
            self.channels[1].first_frame_after_reset = true;
        }
        self.channels[0].n_frames_encoded = 0;
        self.channels[1].n_frames_encoded = 0;
        check_control_input(c)?;
        if prefill == Prefill::No && enc.is_none() {
            return Err(Error::BadArgument("silk: no range encoder"));
        }
        let n_channels_api = c.n_channels_api;
        if !pcm.len().is_multiple_of(n_channels_api) {
            return Err(Error::BadArgument("silk: interleaved input"));
        }
        let mut n_samples_in = pcm.len() / n_channels_api;
        c.switch_ready = false;

        if c.n_channels_internal > self.n_channels_internal {
            // Mono to stereo: a fresh second channel and stereo state.
            self.channels[1] = ChannelEncoder::new();
            self.stereo.pred_prev_q13 = [0; 2];
            self.stereo.s_side = [0; 2];
            self.stereo.mid_side_amp_q0 = [0, 1, 0, 1];
            self.stereo.width_prev_q14 = 0;
            self.stereo.smth_width_q14 = fix_f64(1.0, 14) as i16;
            if self.n_channels_api == 2 {
                self.channels[1].resampler = self.channels[0].resampler.clone();
                self.channels[1].in_hp_state = self.channels[0].in_hp_state;
            }
        }
        let transition = c.payload_size_ms != self.channels[0].packet_size_ms
            || self.n_channels_internal != c.n_channels_internal;
        self.n_channels_api = c.n_channels_api;
        self.n_channels_internal = c.n_channels_internal;
        let n_blocks_of_10ms = (100 * n_samples_in as i32) / c.api_sample_rate;
        let tot_blocks = if n_blocks_of_10ms > 1 {
            n_blocks_of_10ms >> 1
        } else {
            1
        };
        let mut curr_block = 0;
        let (mut tmp_payload_size_ms, mut tmp_complexity) = (0, 0);
        if prefill != Prefill::No {
            if n_blocks_of_10ms != 1 {
                return Err(Error::BadArgument("silk: prefill takes 10 ms"));
            }
            let save_lp = saved_lp(&self.channels[0]);
            for n in 0..c.n_channels_internal {
                self.channels[n] = ChannelEncoder::new();
                if prefill == Prefill::KeepLowPass {
                    self.channels[n].s_lp = save_lp;
                }
            }
            tmp_payload_size_ms = c.payload_size_ms;
            c.payload_size_ms = 10;
            tmp_complexity = c.complexity;
            c.complexity = 0;
            for n in 0..c.n_channels_internal {
                self.channels[n].controlled_since_last_payload = false;
                self.channels[n].prefill_flag = true;
            }
        } else if n_blocks_of_10ms * c.api_sample_rate != 100 * n_samples_in as i32
            || 1000 * n_samples_in as i32 > c.payload_size_ms * c.api_sample_rate
        {
            return Err(Error::BadArgument("silk: input length"));
        }
        let prefill_flag = prefill != Prefill::No;

        for n in 0..c.n_channels_internal {
            let force_fs_khz = if n == 1 { self.channels[0].fs_khz } else { 0 };
            let allow = self.allow_bandwidth_switch;
            control_encoder(&mut self.channels[n], c, allow, n, force_fs_khz);
            if self.channels[n].first_frame_after_reset || transition {
                let nfpp = self.channels[0].n_frames_per_packet;
                self.channels[n].lbrr_flags[..nfpp].fill(false);
            }
            self.channels[n].in_dtx = self.channels[n].use_dtx;
        }

        let fs_khz = self.channels[0].fs_khz as usize;
        let n_samples_to_buffer_max = 10 * n_blocks_of_10ms as usize * fs_khz;
        let mut n_bytes_out = 0usize;
        loop {
            let mut curr_n_bits_used_lbrr = 0;
            let st0 = &self.channels[0];
            let n_samples_to_buffer =
                (st0.frame_length - st0.input_buf_ix).min(n_samples_to_buffer_max);
            let n_samples_from_input = n_samples_to_buffer * 48 / fs_khz;
            if n_samples_from_input > n_samples_in {
                return Err(Error::BadArgument("silk: input length"));
            }
            let buf = &mut self.buf[..n_samples_from_input];
            if c.n_channels_api == 2 && c.n_channels_internal == 2 {
                let id = self.channels[0].n_frames_encoded;
                for (n, b) in buf.iter_mut().enumerate() {
                    *b = pcm[2 * n];
                }
                if self.n_prev_channels_internal == 1 && id == 0 {
                    self.channels[1].resampler = self.channels[0].resampler.clone();
                }
                let st0 = &mut self.channels[0];
                let ix = st0.input_buf_ix;
                st0.resampler.process(
                    &mut st0.input_buf[ix + 2..ix + 2 + n_samples_to_buffer],
                    buf,
                );
                st0.input_buf_ix += n_samples_to_buffer;

                let st1 = &mut self.channels[1];
                let n_to_buffer1 = (st1.frame_length - st1.input_buf_ix)
                    .min(10 * n_blocks_of_10ms as usize * st1.fs_khz as usize);
                for (n, b) in buf.iter_mut().enumerate() {
                    *b = pcm[2 * n + 1];
                }
                let ix = st1.input_buf_ix;
                st1.resampler
                    .process(&mut st1.input_buf[ix + 2..ix + 2 + n_to_buffer1], buf);
                st1.input_buf_ix += n_to_buffer1;
            } else if c.n_channels_api == 2 && c.n_channels_internal == 1 {
                for (n, b) in buf.iter_mut().enumerate() {
                    let sum = i32::from(pcm[2 * n]) + i32::from(pcm[2 * n + 1]);
                    *b = fix::rshift_round(sum, 1) as i16;
                }
                let st0 = &mut self.channels[0];
                let ix = st0.input_buf_ix;
                st0.resampler.process(
                    &mut st0.input_buf[ix + 2..ix + 2 + n_samples_to_buffer],
                    buf,
                );
                // On the first mono frame, average the two resamplers' outputs.
                if self.n_prev_channels_internal == 2 && self.channels[0].n_frames_encoded == 0 {
                    let [st0, st1] = &mut *self.channels;
                    let ix1 = st1.input_buf_ix;
                    st1.resampler.process(
                        &mut st1.input_buf[ix1 + 2..ix1 + 2 + n_samples_to_buffer],
                        buf,
                    );
                    for n in 0..st0.frame_length {
                        let a = i32::from(st0.input_buf[ix + n + 2]);
                        let b = i32::from(st1.input_buf[ix1 + n + 2]);
                        st0.input_buf[ix + n + 2] = ((a + b) >> 1) as i16;
                    }
                }
                self.channels[0].input_buf_ix += n_samples_to_buffer;
            } else {
                buf.copy_from_slice(&pcm[..n_samples_from_input]);
                let st0 = &mut self.channels[0];
                let ix = st0.input_buf_ix;
                st0.resampler.process(
                    &mut st0.input_buf[ix + 2..ix + 2 + n_samples_to_buffer],
                    buf,
                );
                st0.input_buf_ix += n_samples_to_buffer;
            }
            pcm = &pcm[n_samples_from_input * n_channels_api..];
            n_samples_in -= n_samples_from_input;

            self.allow_bandwidth_switch = false;
            if self.channels[0].input_buf_ix < self.channels[0].frame_length {
                break;
            }
            let nfe = self.channels[0].n_frames_encoded;
            let nfpp = self.channels[0].n_frames_per_packet;
            let nci = c.n_channels_internal;

            if nfe == 0 && !prefill_flag {
                let enc = enc.as_deref_mut().expect("checked above");
                // Room for the VAD and LBRR flags, patched in at the end.
                let icdf = [(256 - (256 >> ((nfpp + 1) * nci))) as u8, 0];
                enc.enc_icdf(0, &icdf, 8);
                curr_n_bits_used_lbrr = enc.tell();
                for n in 0..nci {
                    let st = &mut self.channels[n];
                    let mut symbol = 0usize;
                    for i in 0..st.n_frames_per_packet {
                        symbol |= usize::from(st.lbrr_flags[i]) << i;
                    }
                    st.lbrr_flag = symbol > 0;
                    if symbol > 0 && st.n_frames_per_packet > 1 {
                        enc.enc_icdf(symbol - 1, LBRR_FLAGS_ICDF[st.n_frames_per_packet - 2], 8);
                    }
                }
                for i in 0..nfpp {
                    for n in 0..nci {
                        if self.channels[n].lbrr_flags[i] {
                            if nci == 2 && n == 0 {
                                encode_pred(enc, &self.stereo.pred_ix[i]);
                                if !self.channels[1].lbrr_flags[i] {
                                    encode_mid_only(enc, self.stereo.mid_only_flags[i]);
                                }
                            }
                            let cond = if i > 0 && self.channels[n].lbrr_flags[i - 1] {
                                CondCoding::Conditionally
                            } else {
                                CondCoding::Independently
                            };
                            encode_lbrr_frame(&mut self.channels[n], enc, i, cond);
                        }
                    }
                }
                for n in 0..nci {
                    self.channels[n].lbrr_flags = [false; MAX_FRAMES_PER_PACKET];
                }
                curr_n_bits_used_lbrr = enc.tell() - curr_n_bits_used_lbrr;
            }

            hp_variable_cutoff(&mut self.channels[0]);

            let mut n_bits = c.bit_rate * c.payload_size_ms / 1000;
            if !prefill_flag {
                // A moving average of the LBRR's bits, reset when a packet has none.
                if curr_n_bits_used_lbrr < 10 {
                    self.n_bits_used_lbrr = 0;
                } else if self.n_bits_used_lbrr < 10 {
                    self.n_bits_used_lbrr = curr_n_bits_used_lbrr;
                } else {
                    self.n_bits_used_lbrr = (self.n_bits_used_lbrr + curr_n_bits_used_lbrr) / 2;
                }
                n_bits -= self.n_bits_used_lbrr;
            }
            n_bits /= nfpp as i32;
            let mut target_rate_bps = if c.payload_size_ms == 10 {
                smulbb(n_bits, 100)
            } else {
                smulbb(n_bits, 50)
            };
            target_rate_bps -= self.n_bits_exceeded * 1000 / BITRESERVOIR_DECAY_TIME_MS;
            if !prefill_flag && nfe > 0 {
                let tell = enc.as_deref().map_or(0, RangeEncoder::tell);
                let bits_balance = tell - self.n_bits_used_lbrr - n_bits * nfe as i32;
                target_rate_bps -= bits_balance * 1000 / BITRESERVOIR_DECAY_TIME_MS;
            }
            target_rate_bps = fix::limit(target_rate_bps, c.bit_rate, 5000);

            let mut ms_target_rates_bps = [0i32; 2];
            if nci == 2 {
                let [st0, st1] = &mut *self.channels;
                let choice = lr_to_ms(
                    &mut self.stereo,
                    &mut st0.input_buf,
                    &mut st1.input_buf,
                    nfe,
                    target_rate_bps,
                    st0.speech_activity_q8,
                    c.to_mono,
                    st0.fs_khz as usize,
                    st0.frame_length,
                );
                ms_target_rates_bps = choice.mid_side_rates_bps;
                if !self.stereo.mid_only_flags[nfe] {
                    if self.prev_decode_only_middle {
                        // The side channel's first frame after mid-only frames.
                        st1.s_shape = ShapeState::default();
                        st1.s_nsq = NsqState::default();
                        st1.prev_nlsfq_q15 = [0; 16];
                        st1.s_lp.in_lp_state = [0; 2];
                        st1.prev_lag = 100;
                        st1.s_nsq.lag_prev = 100;
                        st1.s_shape.last_gain_index = 10;
                        st1.prev_signal_type = TYPE_NO_VOICE_ACTIVITY;
                        st1.s_nsq.prev_gain_q16 = 65536;
                        st1.first_frame_after_reset = true;
                    }
                    encode_do_vad(st1, activity);
                } else {
                    st1.vad_flags[nfe] = false;
                }
                if !prefill_flag {
                    let enc = enc.as_deref_mut().expect("checked above");
                    encode_pred(enc, &self.stereo.pred_ix[nfe]);
                    if !self.channels[1].vad_flags[nfe] {
                        encode_mid_only(enc, self.stereo.mid_only_flags[nfe]);
                    }
                }
            } else {
                // The mid channel's two samples of history.
                let st0 = &mut self.channels[0];
                st0.input_buf[..2].copy_from_slice(&self.stereo.s_mid);
                let fl = st0.frame_length;
                self.stereo
                    .s_mid
                    .copy_from_slice(&st0.input_buf[fl..fl + 2]);
            }
            encode_do_vad(&mut self.channels[0], activity);

            for n in 0..nci {
                let mut max_bits = c.max_bits;
                if tot_blocks == 2 && curr_block == 0 {
                    max_bits = max_bits * 3 / 5;
                } else if tot_blocks == 3 {
                    if curr_block == 0 {
                        max_bits = max_bits * 2 / 5;
                    } else if curr_block == 1 {
                        max_bits = max_bits * 3 / 4;
                    }
                }
                let mut use_cbr = c.use_cbr && curr_block == tot_blocks - 1;
                let channel_rate_bps = if nci == 1 {
                    target_rate_bps
                } else {
                    if n == 0 && ms_target_rates_bps[1] > 0 {
                        use_cbr = false;
                        // The mid gets up to half the frame's bits.
                        max_bits -= c.max_bits / (tot_blocks * 2);
                    }
                    ms_target_rates_bps[n]
                };
                if channel_rate_bps > 0 {
                    control_snr(&mut self.channels[n], channel_rate_bps);
                    // Channel 0 has counted this frame already when channel 1 codes it.
                    let frames_done = self.channels[0].n_frames_encoded as i32;
                    let cond = if frames_done - n as i32 <= 0 {
                        CondCoding::Independently
                    } else if n > 0 && self.prev_decode_only_middle {
                        // A side frame was skipped in this packet: the LTP state is known.
                        CondCoding::IndependentlyNoLtpScaling
                    } else {
                        CondCoding::Conditionally
                    };
                    let st = &mut self.channels[n];
                    n_bytes_out = match enc.as_deref_mut() {
                        Some(e) => encode_frame(st, e, cond, max_bits, use_cbr),
                        None => {
                            let mut scratch = RangeEncoder::new(1);
                            encode_frame(st, &mut scratch, cond, max_bits, use_cbr)
                        }
                    };
                }
                let st = &mut self.channels[n];
                st.controlled_since_last_payload = false;
                st.input_buf_ix = 0;
                st.n_frames_encoded += 1;
            }
            let nfe = self.channels[0].n_frames_encoded;
            self.prev_decode_only_middle = self.stereo.mid_only_flags[nfe - 1];

            if n_bytes_out > 0 && nfe == nfpp {
                let mut flags = 0u32;
                for n in 0..nci {
                    let st = &self.channels[n];
                    for i in 0..st.n_frames_per_packet {
                        flags = (flags << 1) | u32::from(st.vad_flags[i]);
                    }
                    flags = (flags << 1) | u32::from(st.lbrr_flag);
                }
                if !prefill_flag {
                    let enc = enc.as_deref_mut().expect("checked above");
                    enc.patch_initial_bits(flags, ((nfpp + 1) * nci) as u32);
                }
                if self.channels[0].in_dtx && (nci == 1 || self.channels[1].in_dtx) {
                    n_bytes_out = 0;
                }
                self.n_bits_exceeded += n_bytes_out as i32 * 8;
                self.n_bits_exceeded -= c.bit_rate * c.payload_size_ms / 1000;
                self.n_bits_exceeded = self.n_bits_exceeded.clamp(0, 10000);
                let thr_q8 = smlawb(
                    fix_f32(SPEECH_ACTIVITY_DTX_THRES, 8),
                    fix_f32(
                        (1.0 - SPEECH_ACTIVITY_DTX_THRES) / MAX_BANDWIDTH_SWITCH_DELAY_MS,
                        16 + 8,
                    ),
                    self.time_since_switch_allowed_ms,
                );
                if self.channels[0].speech_activity_q8 < thr_q8 {
                    self.allow_bandwidth_switch = true;
                    self.time_since_switch_allowed_ms = 0;
                } else {
                    self.allow_bandwidth_switch = false;
                    self.time_since_switch_allowed_ms += c.payload_size_ms;
                }
            }
            if n_samples_in == 0 {
                break;
            }
            curr_block += 1;
        }

        self.n_prev_channels_internal = c.n_channels_internal;
        let st0 = &self.channels[0];
        c.allow_bandwidth_switch = self.allow_bandwidth_switch;
        c.in_wb_mode_without_variable_lp = st0.fs_khz == 16 && st0.s_lp.mode == 0;
        c.internal_sample_rate = st0.fs_khz * 1000;
        c.stereo_width_q14 = if c.to_mono {
            0
        } else {
            self.stereo.smth_width_q14.into()
        };
        if prefill_flag {
            c.payload_size_ms = tmp_payload_size_ms;
            c.complexity = tmp_complexity;
            for n in 0..c.n_channels_internal {
                self.channels[n].controlled_since_last_payload = false;
                self.channels[n].prefill_flag = false;
            }
        }
        let st0 = &self.channels[0];
        c.signal_type = st0.indices.signal_type.into();
        c.offset = i32::from(
            QUANTIZATION_OFFSETS_Q10[(st0.indices.signal_type >> 1) as usize]
                [st0.indices.quant_offset_type as usize],
        );
        Ok(n_bytes_out)
    }
}

/// `variable_HP_smth2_Q15`'s start (`opus_encoder_init`) and its value in CELT-only mode: the
/// lowest cutoff, 60 Hz, as a log in Q15.
pub(crate) fn hp_min_cutoff_smth_q15() -> i32 {
    fix::lshift(lin2log(VARIABLE_HP_MIN_CUTOFF_HZ), 8)
}

/// The Opus layer's second smoothing of SILK's high-pass cutoff (`variable_HP_smth2_Q15`
/// following `smth1`); returns the new state and its cutoff in Hz.
pub(crate) fn hp_smooth_cutoff(smth2_q15: i32, smth1_q15: i32) -> (i32, i32) {
    const VARIABLE_HP_SMTH_COEF2: f32 = 0.015;
    let smth2 = smlawb(
        smth2_q15,
        smth1_q15 - smth2_q15,
        fix_f32(VARIABLE_HP_SMTH_COEF2, 16),
    );
    (smth2, crate::silk::macros::log2lin(smth2 >> 8))
}

/// `hp_cutoff`'s biquad (`opus_encoder.c`) in Q28: the numerator and the denominator's last
/// two taps of a second-order high-pass at `cutoff_hz` for 48 kHz.
#[allow(clippy::approx_constant)] // the reference's 3.14159
pub(crate) fn hp_cutoff_coefs(cutoff_hz: i32) -> ([i32; 3], [i32; 2]) {
    let fc_q19 = fix::div32_16(smulbb(fix_f64(1.5 * 3.14159 / 1000.0, 19), cutoff_hz), 48);
    let r_q28 = fix_f64(1.0, 28) - fix_f64(0.92, 9) * fc_q19;
    let b = [r_q28, -r_q28 << 1, r_q28];
    let r_q22 = r_q28 >> 6;
    let smulww = |a: i32, b: i32| ((i64::from(a) * i64::from(b)) >> 16) as i32;
    let a = [
        smulww(r_q22, smulww(fc_q19, fc_q19) - fix_f64(2.0, 22)),
        smulww(r_q22, r_q22),
    ];
    (b, a)
}

/// `silk_HP_variable_cutoff`: the Opus layer's high-pass cutoff follows the low end of the
/// pitch range.
fn hp_variable_cutoff(st: &mut ChannelEncoder) {
    if st.prev_signal_type != TYPE_VOICED {
        return;
    }
    let min_log = lin2log(fix_f64(f64::from(VARIABLE_HP_MIN_CUTOFF_HZ), 16)) - (16 << 7);
    let pitch_freq_hz_q16 = fix::div32((st.fs_khz * 1000) << 16, st.prev_lag);
    let mut pitch_freq_log_q7 = lin2log(pitch_freq_hz_q16) - (16 << 7);
    let quality_q15 = st.input_quality_bands_q15[0];
    pitch_freq_log_q7 = smlawb(
        pitch_freq_log_q7,
        fix::smulwb(fix::lshift(-quality_q15, 2), quality_q15),
        pitch_freq_log_q7 - min_log,
    );
    let mut delta_freq_q7 = pitch_freq_log_q7 - (st.variable_hp_smth1_q15 >> 8);
    if delta_freq_q7 < 0 {
        delta_freq_q7 *= 3;
    }
    let max_delta = fix_f32(VARIABLE_HP_MAX_DELTA_FREQ, 7);
    delta_freq_q7 = delta_freq_q7.clamp(-max_delta, max_delta);
    st.variable_hp_smth1_q15 = smlawb(
        st.variable_hp_smth1_q15,
        smulbb(st.speech_activity_q8, delta_freq_q7),
        fix_f32(VARIABLE_HP_SMTH_COEF1, 16),
    );
    st.variable_hp_smth1_q15 = st.variable_hp_smth1_q15.clamp(
        fix::lshift(lin2log(VARIABLE_HP_MIN_CUTOFF_HZ), 8),
        fix::lshift(lin2log(VARIABLE_HP_MAX_CUTOFF_HZ), 8),
    );
}

#[cfg(test)]
#[path = "enc_tests.rs"]
mod tests;
