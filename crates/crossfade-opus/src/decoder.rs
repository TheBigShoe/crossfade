//! The top-level Opus decoder: a port of libopus `src/opus_decoder.c` (`opus_decode_native`
//! and `opus_decode_frame`, the float build, without deep PLC, OSCE or DRED; 3-clause BSD, see
//! the crate's `COPYING`), which drives the SILK and CELT layers over one shared range decoder
//! per frame.
//!
//! Per frame, as `opus_decode_frame` does:
//!
//! - **SILK** (SILK-only and hybrid) decodes first, at 8, 12 or 16 kHz (always 16 in hybrid),
//!   into the output.
//! - **Redundancy.** Then, if `tell + 17 (+ 20 in hybrid) <= 8 * len`, a redundant 5 ms CELT
//!   frame may follow (its flag is `dec_bit_logp(12)` in hybrid and implied in SILK-only
//!   mode): `celt_to_silk`, and its length (`dec_uint(256) + 2` in hybrid, the rest of the
//!   frame in SILK-only), taken off the end of the frame. It is decoded with its own range
//!   decoder: before the main CELT call when going from CELT to SILK (its first 2.5 ms replace
//!   the output's and the next 2.5 ms are cross-faded), after it, from a reset CELT, when
//!   going from SILK to CELT (cross-faded into the frame's last 2.5 ms). The final range is
//!   the frame's `rng ^ redundant_rng`.
//! - **CELT** (CELT-only, and hybrid from band 17, adding onto SILK) decodes last. Going from
//!   hybrid to SILK-only, CELT fades out by decoding the silence frame `[0xFF, 0xFF]`.
//! - **Transitions without redundancy** (CELT to SILK or hybrid, or SILK or hybrid to CELT
//!   without a redundant frame before) cross-fade 5 ms of the previous mode's concealment into
//!   the frame's first 2.5 ms; CELT is reset on a mode change unless the last frame ended with
//!   redundancy, SILK when it follows CELT.
//! - **Concealment.** A lost packet ([`Decoder::decode`] with `None`) conceals the last
//!   packet's duration with the last frame's mode (CELT if it ended with redundancy), in calls
//!   of at most 20 ms rounded as the reference rounds them; a frame of 0 or 1 bytes conceals
//!   its own duration. Before any audio, concealment is silence. [`Decoder::decode_fec`]
//!   decodes a packet's forward error correction (SILK's LBRR data) in place of the lost one
//!   before it.
//! - **Gain** ([`Decoder::set_gain`], `OPUS_SET_GAIN`) scales every frame's output.
//!
//! Output is always 48 kHz, interleaved `f32`, full scale 1.0 (`opus_decode_float`: no soft
//! clipping), `channels` channels whatever the packets code: the layers up- or down-mix.

use crate::celt::{CeltDecoder, CeltFrame, OVERLAP, WINDOW};
use crate::error::{Error, Result};
use crate::packet::{Bandwidth, MAX_PACKET_SAMPLES, Mode, Packet};
use crate::range::RangeDecoder;
use crate::silk::{SilkDecoder, SilkFrame};

/// Frame sizes at 48 kHz: 2.5, 5, 10 and 20 ms.
const F2_5: usize = 120;
const F5: usize = 240;
const F10: usize = 480;
const F20: usize = 960;

/// An Opus decoder for one stream (not multistream), always at 48 kHz.
#[derive(Debug, Clone)]
pub struct Decoder {
    channels: usize,
    celt: CeltDecoder,
    silk: SilkDecoder,
    /// `OPUS_SET_GAIN`, Q8 dB; kept by [`reset`](Self::reset), as in libopus.
    gain: i16,
    /// CELT's end band as last set (`CELT_SET_END_BAND`): concealment keeps it.
    celt_end_band: usize,
    // From here on, what OPUS_RESET_STATE clears.
    /// Coded channels of the last packet (`st->stream_channels`).
    stream_channels: usize,
    /// The last packet's bandwidth, mode and frame size (`st->bandwidth`, `mode`,
    /// `frame_size`).
    bandwidth: Bandwidth,
    mode: Option<Mode>,
    frame_size: usize,
    /// The mode of the last frame decoded; `None` before any (libopus `prev_mode == 0`).
    prev_mode: Option<Mode>,
    /// Whether the last frame ended with a redundant CELT frame (SILK to CELT).
    prev_redundancy: bool,
    last_packet_duration: usize,
    final_range: u32,
}

/// `smooth_fade` (float build) in place: `out[i] = w * in2[i] + (1 - w) * in1[i]` over
/// [`F2_5`] samples per channel, `w` the squared CELT window; `out` is `in1`
/// (`out_is_in2` false) or `in2`.
fn smooth_fade(out: &mut [f32], other: &[f32], out_is_in2: bool, channels: usize) {
    for c in 0..channels {
        for (i, &win) in WINDOW.iter().enumerate().take(F2_5.min(OVERLAP)) {
            let w = win * win;
            let k = i * channels + c;
            let (in1, in2) = if out_is_in2 {
                (other[k], out[k])
            } else {
                (out[k], other[k])
            };
            out[k] = (w * in2) + ((1.0 - w) * in1);
        }
    }
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
            gain: 0,
            celt_end_band: Bandwidth::Full.celt_end_band(),
            stream_channels: channels,
            bandwidth: Bandwidth::Full,
            mode: None,
            frame_size: F2_5,
            prev_mode: None,
            prev_redundancy: false,
            last_packet_duration: 0,
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

    /// The duration of the last packet decoded or concealed, in samples at 48 kHz
    /// (`OPUS_GET_LAST_PACKET_DURATION`): what a lost packet conceals.
    pub fn last_packet_duration(&self) -> usize {
        self.last_packet_duration
    }

    /// Forgets the stream, as if new (`OPUS_RESET_STATE`); the gain is kept.
    pub fn reset(&mut self) {
        self.celt.reset();
        self.silk.reset();
        self.stream_channels = self.channels;
        self.bandwidth = Bandwidth::Full;
        self.mode = None;
        self.frame_size = F2_5;
        self.prev_mode = None;
        self.prev_redundancy = false;
        self.last_packet_duration = 0;
        self.final_range = 0;
    }

    /// Decodes one packet (`None`: a lost packet, concealed), appending its frames at 48 kHz,
    /// interleaved, to `out`, and returns how many frames it appended. An error appends
    /// nothing; an invalid packet also changes nothing.
    pub fn decode(&mut self, packet: Option<&[u8]>, out: &mut Vec<f32>) -> Result<usize> {
        let start = out.len();
        let result = match packet {
            None => self.conceal_frames(self.last_packet_duration, out),
            Some(data) => Packet::parse(data).and_then(|p| self.decode_packet(&p, out)),
        };
        if result.is_err() {
            out.truncate(start);
        }
        result
    }

    /// Decodes `packet`'s forward error correction (SILK's LBRR frames) in place of the lost
    /// packet before it, appending the lost packet's duration (the last packet's, as for
    /// [`decode`](Self::decode) with `None`); decode `packet` itself next. Where it carries
    /// none (CELT, or a frame longer than the loss) this conceals, as libopus does (decode
    /// with `decode_fec = 1`). An invalid packet is an error that changes nothing.
    pub fn decode_fec(&mut self, packet: &[u8], out: &mut Vec<f32>) -> Result<usize> {
        let parsed = Packet::parse(packet)?;
        self.decode_fec_packet(&parsed, out)
    }

    /// [`decode_fec`](Self::decode_fec) of a packet already parsed.
    pub(crate) fn decode_fec_packet(
        &mut self,
        parsed: &Packet<'_>,
        out: &mut Vec<f32>,
    ) -> Result<usize> {
        let frames = self.last_packet_duration;
        let start = out.len();
        let result = self.fec(parsed, frames, out);
        if result.is_err() {
            out.truncate(start);
        }
        result
    }

    fn fec(&mut self, packet: &Packet<'_>, frames: usize, out: &mut Vec<f32>) -> Result<usize> {
        let toc = packet.toc();
        let packet_frame = toc.frame_samples();
        if frames < packet_frame || toc.mode() == Mode::Celt || self.mode == Some(Mode::Celt) {
            return self.conceal_frames(frames, out);
        }
        // Conceal all but the last frame's worth, which the LBRR data stands in for.
        let duration = self.last_packet_duration;
        if frames > packet_frame
            && let Err(e) = self.conceal_frames(frames - packet_frame, out)
        {
            self.last_packet_duration = duration;
            return Err(e);
        }
        self.mode = Some(toc.mode());
        self.bandwidth = toc.bandwidth();
        self.frame_size = packet_frame;
        self.stream_channels = if toc.stereo() { 2 } else { 1 };
        let at = out.len();
        out.resize(at + packet_frame * self.channels, 0.0);
        self.decode_frame(Some(packet.frames()[0]), &mut out[at..], packet_frame, true)?;
        self.last_packet_duration = frames;
        Ok(frames)
    }

    /// Sets the output gain in Q8 dB (`OPUS_SET_GAIN`; the Ogg Opus header's output gain).
    pub fn set_gain(&mut self, gain: i16) {
        self.gain = gain;
    }

    /// The output gain in Q8 dB.
    pub fn gain(&self) -> i16 {
        self.gain
    }

    /// Conceals `frames` samples (a whole number of 2.5 ms, at most 120 ms) as if a packet
    /// that long were lost (`opus_decode` with no data): silence before any audio. Appends
    /// them to `out`; an error appends nothing.
    pub fn conceal(&mut self, frames: usize, out: &mut Vec<f32>) -> Result<usize> {
        if frames == 0 || !frames.is_multiple_of(F2_5) || frames > MAX_PACKET_SAMPLES {
            return Err(Error::BadArgument(
                "concealment is a whole number of 2.5 ms, at most 120 ms",
            ));
        }
        let start = out.len();
        let result = self.conceal_frames(frames, out);
        if result.is_err() {
            out.truncate(start);
        }
        result
    }

    /// `opus_decode_native` with no data: `frames` concealed, frame by frame.
    fn conceal_frames(&mut self, frames: usize, out: &mut Vec<f32>) -> Result<usize> {
        let start = out.len();
        out.resize(start + frames * self.channels, 0.0);
        let mut count = 0;
        while count < frames {
            let at = start + count * self.channels;
            count += self.decode_frame(None, &mut out[at..], frames - count, false)?;
        }
        self.last_packet_duration = count;
        Ok(count)
    }

    /// [`decode`](Self::decode) of a packet already parsed (a multistream packet's streams are
    /// self-delimited); on an error the caller truncates `out`.
    pub(crate) fn decode_packet(
        &mut self,
        packet: &Packet<'_>,
        out: &mut Vec<f32>,
    ) -> Result<usize> {
        let toc = packet.toc();
        self.mode = Some(toc.mode());
        self.bandwidth = toc.bandwidth();
        self.frame_size = toc.frame_samples();
        self.stream_channels = if toc.stereo() { 2 } else { 1 };
        let start = out.len();
        let total = packet.duration();
        out.resize(start + total * self.channels, 0.0);
        let mut count = 0;
        for frame in packet.frames() {
            let at = start + count * self.channels;
            count += self.decode_frame(Some(frame), &mut out[at..], total - count, false)?;
        }
        self.last_packet_duration = count;
        Ok(count)
    }

    /// `opus_decode_frame`: one frame (`None`: conceal `frame_size` samples) into the start
    /// of `pcm`, which has room for `frame_size`; returns the samples written.
    fn decode_frame(
        &mut self,
        data: Option<&[u8]>,
        pcm: &mut [f32],
        frame_size: usize,
        fec: bool,
    ) -> Result<usize> {
        let ch = self.channels;
        if frame_size < F2_5 {
            return Err(Error::BadArgument("an Opus frame is at least 2.5 ms"));
        }
        let mut frame_size = frame_size.min(MAX_PACKET_SAMPLES);
        // Payloads of 0 or 1 bytes are concealed, no longer than the TOC says.
        let data = data.filter(|d| d.len() > 1);
        if data.is_none() {
            frame_size = frame_size.min(self.frame_size);
        }
        let mut len = data.map_or(0, |d| d.len() as i64);
        let mut dec = RangeDecoder::new(data.unwrap_or(&[]));
        let (audiosize, mode, bandwidth) = match data {
            Some(_) => (
                self.frame_size,
                self.mode.unwrap_or(Mode::Celt),
                Some(self.bandwidth),
            ),
            None => {
                // Conceal with the last mode (CELT if the last frame ended with redundancy).
                let mode = if self.prev_redundancy {
                    Some(Mode::Celt)
                } else {
                    self.prev_mode
                };
                let Some(mode) = mode else {
                    pcm[..frame_size * ch].fill(0.0);
                    return Ok(frame_size);
                };
                let mut audiosize = frame_size;
                // Concealment runs on 2.5 (CELT), 5 (CELT), 10 or 20 ms only.
                if audiosize > F20 {
                    let mut at = 0;
                    while audiosize > 0 {
                        let n = self.decode_frame(
                            None,
                            &mut pcm[at * ch..],
                            audiosize.min(F20),
                            false,
                        )?;
                        at += n;
                        audiosize -= n;
                    }
                    return Ok(frame_size);
                } else if audiosize < F20 {
                    if audiosize > F10 {
                        audiosize = F10;
                    } else if mode != Mode::Silk && audiosize > F5 && audiosize < F10 {
                        audiosize = F5;
                    }
                }
                (audiosize, mode, None)
            }
        };
        let celt_accum = mode != Mode::Celt;

        let mut transition = data.is_some()
            && self.prev_mode.is_some_and(|prev| {
                (mode == Mode::Celt && prev != Mode::Celt && !self.prev_redundancy)
                    || (mode != Mode::Celt && prev == Mode::Celt)
            });
        let mut pcm_transition = Vec::new();
        if transition && mode == Mode::Celt {
            pcm_transition = self.conceal_transition(F5.min(audiosize))?;
        }
        if audiosize > frame_size {
            return Err(Error::BadArgument("an Opus frame longer than its buffer"));
        }
        frame_size = audiosize;
        let pcm = &mut pcm[..frame_size * ch];

        if mode != Mode::Celt {
            if self.prev_mode == Some(Mode::Celt) {
                self.silk.reset();
            }
            let internal_rate = match (mode, bandwidth) {
                (Mode::Silk, Some(Bandwidth::Narrow)) => 8000,
                (Mode::Silk, Some(Bandwidth::Medium)) => 12000,
                _ => 16000,
            };
            let silk = SilkFrame {
                frame_size,
                internal_rate,
                stream_channels: self.stream_channels,
                fec,
            };
            let lost = data.is_none() || fec;
            let dec = data.is_some().then_some(&mut dec);
            // Concealment that fails is silence, not an error (SILK writes the zeros).
            if let Err(e) = self.silk.decode(dec, &silk, pcm)
                && !lost
            {
                return Err(e);
            }
        }

        let mut redundancy = false;
        let mut celt_to_silk = false;
        let mut redundancy_bytes = 0i64;
        if !fec
            && mode != Mode::Celt
            && data.is_some()
            && i64::from(dec.tell()) + 17 + 20 * i64::from(mode == Mode::Hybrid) <= 8 * len
        {
            redundancy = mode == Mode::Silk || dec.dec_bit_logp(12);
            if redundancy {
                celt_to_silk = dec.dec_bit_logp(1);
                redundancy_bytes = if mode == Mode::Hybrid {
                    i64::from(dec.dec_uint(256)) + 2
                } else {
                    len - ((i64::from(dec.tell()) + 7) >> 3)
                };
                len -= redundancy_bytes;
                // Not for a valid packet; the reference's behaviour, not normative.
                if len * 8 < i64::from(dec.tell()) {
                    len = 0;
                    redundancy_bytes = 0;
                    redundancy = false;
                }
                dec.shrink(redundancy_bytes as usize);
            }
        }
        let start_band = if mode == Mode::Celt { 0 } else { 17 };
        if redundancy {
            transition = false;
        }
        if transition && mode != Mode::Celt {
            pcm_transition = self.conceal_transition(F5.min(audiosize))?;
        }
        if let Some(bw) = bandwidth {
            self.celt_end_band = bw.celt_end_band();
        }
        let (end_band, stream_channels) = (self.celt_end_band, self.stream_channels);
        let celt_frame = move |frame_size, start_band, accumulate| CeltFrame {
            frame_size,
            start_band,
            end_band,
            stream_channels,
            accumulate,
        };
        // `redundancy` implies data, and 0 <= len, len + redundancy_bytes <= its length.
        let redundant = match data {
            Some(d) if redundancy => &d[len as usize..(len + redundancy_bytes) as usize],
            _ => &[][..],
        };
        let mut redundant_audio = Vec::new();
        let mut redundant_rng = 0;

        if redundancy && celt_to_silk {
            // Decoded even when its audio is not used: the final range needs it.
            redundant_audio = vec![0.0; F5 * ch];
            let mut rdec = RangeDecoder::new(redundant);
            let _ = self.celt.decode(
                Some(&mut rdec),
                &celt_frame(F5, 0, false),
                &mut redundant_audio,
            );
            redundant_rng = self.celt.final_range();
        }

        let mut celt_result = Ok(());
        if mode != Mode::Silk {
            let celt_frame_size = F20.min(frame_size);
            if self.prev_mode.is_some_and(|prev| prev != mode) && !self.prev_redundancy {
                self.celt.reset();
            }
            let dec = (!fec && len > 1).then_some(&mut dec);
            celt_result = self.celt.decode(
                dec,
                &celt_frame(celt_frame_size, start_band, celt_accum),
                &mut pcm[..celt_frame_size * ch],
            );
            self.final_range = self.celt.final_range();
        } else {
            // Hybrid to SILK: CELT fades out by decoding a silence frame.
            if self.prev_mode == Some(Mode::Hybrid)
                && !(redundancy && celt_to_silk && self.prev_redundancy)
            {
                let silence = [0xFF, 0xFF];
                let mut sdec = RangeDecoder::new(&silence);
                let _ = self.celt.decode(
                    Some(&mut sdec),
                    &celt_frame(F2_5, 0, celt_accum),
                    &mut pcm[..F2_5 * ch],
                );
            }
            self.final_range = if data.is_some() { dec.rng() } else { 0 };
        }

        if redundancy && !celt_to_silk {
            self.celt.reset();
            redundant_audio = vec![0.0; F5 * ch];
            let mut rdec = RangeDecoder::new(redundant);
            let _ = self.celt.decode(
                Some(&mut rdec),
                &celt_frame(F5, 0, false),
                &mut redundant_audio,
            );
            redundant_rng = self.celt.final_range();
            let tail = (frame_size - F2_5) * ch;
            smooth_fade(&mut pcm[tail..], &redundant_audio[F2_5 * ch..], false, ch);
        }
        // Ignored when the previous frame did not use CELT (the first redundant frame of a
        // switch from SILK may have been lost).
        if redundancy
            && celt_to_silk
            && (self.prev_mode != Some(Mode::Silk) || self.prev_redundancy)
        {
            pcm[..F2_5 * ch].copy_from_slice(&redundant_audio[..F2_5 * ch]);
            smooth_fade(
                &mut pcm[F2_5 * ch..],
                &redundant_audio[F2_5 * ch..],
                true,
                ch,
            );
        }
        if transition {
            if audiosize >= F5 {
                pcm[..F2_5 * ch].copy_from_slice(&pcm_transition[..F2_5 * ch]);
                smooth_fade(
                    &mut pcm[F2_5 * ch..],
                    &pcm_transition[F2_5 * ch..],
                    true,
                    ch,
                );
            } else {
                // Too short for a clean transition; the reference fades anyway.
                smooth_fade(pcm, &pcm_transition, true, ch);
            }
        }

        if self.gain != 0 {
            // The float build's celt_exp2(6.48814081e-4f * gain) (the same f32), the product in
            // float. The reference applies it per call, so a transition's concealment, already
            // scaled by its own call, is scaled again here; kept, to stay sample for sample.
            let x: f32 = 6.488_141e-4 * f32::from(self.gain);
            let g = libm::exp(std::f64::consts::LN_2 * f64::from(x)) as f32;
            for s in pcm.iter_mut() {
                *s *= g;
            }
        }

        if len <= 1 {
            self.final_range = 0;
        } else {
            self.final_range ^= redundant_rng;
        }
        self.prev_mode = Some(mode);
        self.prev_redundancy = redundancy && !celt_to_silk;
        celt_result.map(|()| audiosize)
    }

    /// The previous mode's concealment of `frames` (at most 5 ms) for a transition.
    fn conceal_transition(&mut self, frames: usize) -> Result<Vec<f32>> {
        let mut buf = vec![0.0; F5 * self.channels];
        self.decode_frame(None, &mut buf, frames, false)?;
        Ok(buf)
    }
}

#[cfg(test)]
#[path = "decoder_tests.rs"]
mod tests;
