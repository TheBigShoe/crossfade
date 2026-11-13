//! The top-level Opus decoder: a port of libopus `src/opus_decoder.c` (`opus_decode_native`
//! and `opus_decode_frame`), which drives the SILK and CELT layers over one shared range
//! decoder per frame.
//!
//! # What it does now (stage 0)
//!
//! [`Decoder::decode`] parses the packet ([`Packet::parse`]; an invalid one is an error that
//! changes nothing), then for each frame:
//!
//! - a frame of 0 or 1 bytes is concealed, as `opus_decode_frame` does with `len <= 1`; before
//!   any audio that is silence of the frame's duration, with a final range of 0;
//! - otherwise it makes a [`RangeDecoder`] over the frame and dispatches by the TOC's mode:
//!   SILK-only to [`SilkDecoder::decode`] (internal rate 8, 12 or 16 kHz from the bandwidth),
//!   CELT-only to [`CeltDecoder::decode`] (start band 0, end band from the bandwidth), each
//!   writing the frame's slice of `out`; the final range is the range decoder's after the
//!   layer returns.
//!
//! Hybrid frames, mode switches and packet loss concealment after audio return
//! [`Error::Unsupported`]: they are stage 3, sketched below. A CELT-only or SILK-only stream
//! (testvectors 01, 07, 11 and 02, 03, 04) needs nothing else, so each layer can be proved on
//! its own through this decoder as soon as it is built.
//!
//! Output is always 48 kHz, interleaved `f32`, full scale 1.0 (`opus_decode_float`, no soft
//! clipping), `channels` channels whatever the packets code: the layers up- or down-mix.
//!
//! # Stage 3 (not built): what the top level adds, following `opus_decode_frame`
//!
//! - **Hybrid.** SILK decodes the frame at 16 kHz (WB) first. Then, if `tell + 17 + 20 <=
//!   8 * len` (`+ 20` because the mode is hybrid), the redundancy flag `dec_bit_logp(12)`;
//!   in SILK-only mode, redundancy is implied when `tell + 17 <= 8 * len`. With redundancy:
//!   `celt_to_silk = dec_bit_logp(1)`, the redundancy length (`dec_uint(256) + 2` in hybrid,
//!   the rest of the frame `len - ((tell + 7) >> 3)` in SILK-only), taken off the end:
//!   `len -= bytes; dec.shrink(bytes)` (and if `len * 8 < tell`, no redundancy and `len = 0`,
//!   so CELT conceals). CELT then decodes with start band 17, accumulating onto SILK.
//! - **Redundancy frames** (5 ms CELT frames at SILK/CELT switches, start band 0, their own
//!   range decoder over the bytes at the end): CELT-to-SILK is decoded BEFORE the main CELT
//!   call and its first 2.5 ms replace the output's, then a 2.5 ms `smooth_fade` with CELT's
//!   window (only when the previous frame used CELT or ended with redundancy); SILK-to-CELT is
//!   decoded AFTER, following an `OPUS_RESET_STATE` of CELT, and cross-faded into the last
//!   2.5 ms. The final range is `dec.rng() ^ redundant_rng`.
//! - **Transitions** without redundancy (CELT to SILK/hybrid, or SILK/hybrid to CELT without a
//!   redundancy frame before): 5 ms of the previous mode's concealment (`opus_decode_frame`
//!   with no data) cross-faded over the first 2.5 ms (`smooth_fade`). CELT is reset
//!   (`OPUS_RESET_STATE`) when the mode changes and the previous frame ended without
//!   redundancy; SILK is reset (`silk_ResetDecoder`) when entering SILK or hybrid from CELT.
//!   Hybrid to SILK lets CELT fade out by decoding the silence frame `[0xFF, 0xFF]` (2.5 ms,
//!   start band 0) into the output. `prev_mode` and `prev_redundancy` (redundancy and not
//!   `celt_to_silk`) are kept per frame.
//! - **Packet loss concealment.** A lost packet ([`Decoder::decode`] with `None`) conceals the
//!   last packet's duration (`OPUS_GET_LAST_PACKET_DURATION`, what `opus_demo` asks for), in
//!   chunks of at most 20 ms, each rounded as `opus_decode_frame` does (to 10 ms, or for
//!   non-SILK modes 5 ms), with the previous mode (CELT if the last frame ended with
//!   redundancy); before any packet, nothing. SILK PLC that fails gives silence. FEC (decoding
//!   the next packet's LBRR data in place of a lost one) is SILK's `fec` flag.
//! - **Output gain.** `OPUS_SET_GAIN` in Q8 dB (the Ogg header's output gain): every sample
//!   times `celt_exp2(6.48814081e-4 * gain)` (float build), `libm` only.
//! - **Multistream** (a separate `MultistreamDecoder`, `opus_multistream_decoder.c`): one
//!   [`Decoder`] per stream, the first `coupled` stereo, the rest mono; every stream's packet
//!   but the last is self-delimited ([`Packet::parse_self_delimited`]); output channels taken
//!   from the decoded ones by the mapping table (255: silence). Channel mapping families 0 and
//!   1 (RFC 7845 5.1.1) are what the Ogg reader needs; the table comes from
//!   [`crate::OpusHead`].
//!
//! The vectors' map (`tests/VECTORS.md`) names which test vectors exercise each of these.

use crate::celt::{CeltDecoder, CeltFrame};
use crate::error::{Error, Result};
use crate::packet::{Bandwidth, Mode, Packet};
use crate::range::RangeDecoder;
use crate::silk::{SilkDecoder, SilkFrame};

/// An Opus decoder for one stream (not multistream), always at 48 kHz.
#[derive(Debug, Clone)]
pub struct Decoder {
    channels: usize,
    celt: CeltDecoder,
    silk: SilkDecoder,
    /// The mode of the last frame decoded; `None` before any (libopus `prev_mode == 0`).
    prev_mode: Option<Mode>,
    final_range: u32,
}

impl Decoder {
    /// A decoder giving `channels` (1 or 2) output channels at 48 kHz, whatever the packets
    /// code.
    pub fn new(channels: usize) -> Result<Self> {
        if !(1..=2).contains(&channels) {
            return Err(Error::BadArgument(
                "an Opus decoder has one or two channels",
            ));
        }
        Ok(Self {
            channels,
            celt: CeltDecoder::new(channels),
            silk: SilkDecoder::new(channels),
            prev_mode: None,
            final_range: 0,
        })
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The range decoder's final state after the last frame decoded (`OPUS_GET_FINAL_RANGE`):
    /// what an encoder reports per packet and the test vectors store. 0 after a concealed frame
    /// and before any.
    pub fn final_range(&self) -> u32 {
        self.final_range
    }

    /// Forgets the stream, as if new (`OPUS_RESET_STATE`).
    pub fn reset(&mut self) {
        self.celt.reset();
        self.silk.reset();
        self.prev_mode = None;
        self.final_range = 0;
    }

    /// Decodes one packet (`None`: a lost packet, concealed), appending its frames at 48 kHz,
    /// interleaved, to `out`, and returns how many frames it appended. An error appends
    /// nothing; an invalid packet also changes nothing.
    pub fn decode(&mut self, packet: Option<&[u8]>, out: &mut Vec<f32>) -> Result<usize> {
        let Some(data) = packet else {
            if self.prev_mode.is_none() {
                return Ok(0);
            }
            return Err(Error::Unsupported(
                "packet loss concealment is not built yet",
            ));
        };
        let packet = Packet::parse(data)?;
        let start = out.len();
        out.resize(start + packet.duration() * self.channels, 0.0);
        let frame_len = packet.toc().frame_samples() * self.channels;
        for (frame, pcm) in packet
            .frames()
            .iter()
            .zip(out[start..].chunks_exact_mut(frame_len))
        {
            if let Err(e) = self.decode_frame(frame, &packet, pcm) {
                out.truncate(start);
                return Err(e);
            }
        }
        Ok(packet.duration())
    }

    fn decode_frame(&mut self, frame: &[u8], packet: &Packet<'_>, pcm: &mut [f32]) -> Result<()> {
        let toc = packet.toc();
        if frame.len() <= 1 {
            if self.prev_mode.is_some() {
                return Err(Error::Unsupported(
                    "packet loss concealment is not built yet",
                ));
            }
            pcm.fill(0.0);
            self.final_range = 0;
            return Ok(());
        }
        let mode = toc.mode();
        if self.prev_mode.is_some_and(|prev| prev != mode) {
            return Err(Error::Unsupported("opus mode switches are not built yet"));
        }
        let frame_size = toc.frame_samples();
        let stream_channels = if toc.stereo() { 2 } else { 1 };
        let mut dec = RangeDecoder::new(frame);
        match mode {
            Mode::Silk | Mode::Hybrid => {
                let internal_rate = match toc.bandwidth() {
                    Bandwidth::Narrow => 8000,
                    Bandwidth::Medium => 12000,
                    _ => 16000,
                };
                let silk = SilkFrame {
                    frame_size,
                    internal_rate,
                    stream_channels,
                    fec: false,
                };
                self.silk.decode(Some(&mut dec), &silk, pcm)?;
                if mode == Mode::Hybrid {
                    return Err(Error::Unsupported("hybrid decoding is not built yet"));
                }
            }
            Mode::Celt => {
                let celt = CeltFrame {
                    frame_size,
                    start_band: 0,
                    end_band: toc.bandwidth().celt_end_band(),
                    stream_channels,
                    accumulate: false,
                };
                self.celt.decode(Some(&mut dec), &celt, pcm)?;
            }
        }
        self.final_range = dec.rng();
        self.prev_mode = Some(mode);
        Ok(())
    }
}

#[cfg(test)]
#[path = "decoder_tests.rs"]
mod tests;
