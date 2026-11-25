//! SILK-only Opus packets from the SILK encoder, for tests (re-exported as
//! `testing::silk_encoder` until the crate has a public encoder): one frame per packet, the
//! TOC chosen from the internal rate SILK reports, the payload ended as libopus ends a
//! SILK-only frame.

use super::{EncControl, Prefill, SilkEncoder};
use crate::error::{Error, Result};
use crate::range_enc::RangeEncoder;

/// The largest Opus packet.
const MAX_PACKET: usize = 1275;

/// What the encoder is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SilkOnlyConfig {
    /// 1 or 2; the input is interleaved.
    pub channels: usize,
    /// The internal rate: 8000 (narrowband), 12000 (mediumband) or 16000 (wideband).
    pub internal_rate: u32,
    /// The packet's duration: 10, 20, 40 or 60 ms.
    pub frame_ms: u32,
    /// Bits per second for the packet's SILK data.
    pub bitrate: u32,
    /// 0 to 10.
    pub complexity: u8,
    /// Constant bitrate.
    pub cbr: bool,
}

/// What SILK reported for the last packet (`silk_EncControlStruct`'s outputs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SilkReport {
    pub internal_rate: u32,
    pub stereo_width_q14: i32,
    pub signal_type: i32,
    pub quant_offset_q10: i32,
    pub switch_ready: bool,
    pub allow_bandwidth_switch: bool,
    pub in_wb_mode_without_variable_lp: bool,
    /// The first frame coded the mid channel only.
    pub mid_only: bool,
    /// The high-pass smoother an Opus encoder's input filter follows.
    pub hp_smth_q15: i32,
}

/// The SILK encoder writing SILK-only Opus packets.
#[derive(Debug, Clone)]
pub struct SilkOnlyEncoder {
    silk: SilkEncoder,
    control: EncControl,
    final_range: u32,
}

impl SilkOnlyEncoder {
    pub fn new(cfg: SilkOnlyConfig) -> Result<Self> {
        let rate = cfg.internal_rate as i32;
        let control = EncControl {
            n_channels_api: cfg.channels,
            n_channels_internal: cfg.channels,
            max_internal_sample_rate: rate,
            min_internal_sample_rate: rate,
            desired_internal_sample_rate: rate,
            payload_size_ms: cfg.frame_ms as i32,
            bit_rate: cfg.bitrate.min(80_000) as i32,
            complexity: i32::from(cfg.complexity),
            use_cbr: cfg.cbr,
            ..EncControl::default()
        };
        super::control::check_control_input(&control)?;
        Ok(Self {
            silk: SilkEncoder::new(),
            control,
            final_range: 0,
        })
    }

    /// Fills the buffers from 10 ms of input after a reset, coding nothing (libopus's
    /// prefill); `keep_low_pass` keeps a bandwidth switch's filter state.
    pub fn prefill(&mut self, pcm: &[i16], keep_low_pass: bool) -> Result<()> {
        let p = if keep_low_pass {
            Prefill::KeepLowPass
        } else {
            Prefill::Reset
        };
        self.silk.encode(&mut self.control, pcm, None, p, None)?;
        Ok(())
    }

    /// One packet from `frame_ms` of 48 kHz input (interleaved for stereo).
    pub fn encode(&mut self, pcm: &[i16]) -> Result<Vec<u8>> {
        if pcm.len() != 48 * self.control.payload_size_ms as usize * self.control.n_channels_api {
            return Err(Error::BadArgument("silk: input length"));
        }
        let mut enc = RangeEncoder::new(MAX_PACKET - 1);
        self.control.max_bits = (MAX_PACKET as i32 - 1) * 8;
        let bytes = self.silk.encode(
            &mut self.control,
            pcm,
            Some(&mut enc),
            Prefill::No,
            Some(true),
        )?;
        let mut packet = vec![self.toc()];
        if bytes == 0 {
            // DTX: the TOC alone.
            self.final_range = 0;
            return Ok(packet);
        }
        let n = ((enc.tell() + 7) >> 3) as usize;
        self.final_range = enc.rng();
        let body = enc
            .done()
            .ok_or(Error::BadArgument("silk: packet too large"))?;
        packet.extend_from_slice(&body[..n]);
        Ok(packet)
    }

    /// The range coder's final state after the last packet, as a decoder reports it.
    pub fn final_range(&self) -> u32 {
        self.final_range
    }

    pub fn report(&self) -> SilkReport {
        let c = &self.control;
        SilkReport {
            internal_rate: c.internal_sample_rate as u32,
            stereo_width_q14: c.stereo_width_q14,
            signal_type: c.signal_type,
            quant_offset_q10: c.offset,
            switch_ready: c.switch_ready,
            allow_bandwidth_switch: c.allow_bandwidth_switch,
            in_wb_mode_without_variable_lp: c.in_wb_mode_without_variable_lp,
            mid_only: self.silk.mid_only(0),
            hp_smth_q15: self.silk.variable_hp_smth1_q15(),
        }
    }

    /// The TOC byte of a one-frame SILK-only packet (RFC 6716 3.1).
    fn toc(&self) -> u8 {
        let bw: u8 = match self.control.internal_sample_rate {
            8000 => 0,
            12000 => 1,
            _ => 2,
        };
        let size: u8 = match self.control.payload_size_ms {
            10 => 0,
            20 => 1,
            40 => 2,
            _ => 3,
        };
        ((bw * 4 + size) << 3) | (u8::from(self.control.n_channels_api == 2) << 2)
    }
}
