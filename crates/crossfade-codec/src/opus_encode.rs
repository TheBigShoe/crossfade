//! Opus encoder adapter, the inverse of the decoder adapter in `opus`: `F32` frames at 48 kHz,
//! mono or stereo, in; one packet per 20 ms (960 frames) out, from `crossfade_opus::Encoder`.
//!
//! Timeline. The first packet is stamped `-pre_skip` (the encoder's lookahead, 312 frames),
//! each next one where the last one's TOC duration ended, so that a decoder's output, trimmed
//! at pts 0, lines up with the input. At `drain` the input is padded with silence and encoded
//! on until the decoder's side has everything that was sent in; the last packet states a
//! shorter duration when its TOC's would run past the end of the input (the Ogg writer cuts the
//! last page there). No input at all gives no packet.
//!
//! The header ([`head`]) is known up front, as the Ogg writer needs it: the pre-skip, the channel
//! count, and the rate of the audio before the graph brought it to 48 kHz, which only
//! the caller knows (`EncodeOptions::input_sample_rate`, else 48000).
//!
//! Surround Opus (mapping family 1) is not written yet: three or more channels are refused.

use crate::EncodeOptions;
use crate::opus::engine_error as map;
use crossfade_core::{AudioFrame, AudioSpec, Encoder, Error, Packet, SampleFormat, Samples};
use crossfade_opus::EncoderConfig;
use std::collections::VecDeque;

fn unsupported(what: impl Into<String>) -> Error {
    Error::Unsupported { what: what.into() }
}

/// The only rate Opus is coded at.
pub const SAMPLE_RATE: u32 = 48_000;

/// The pre-skip every stream this adapter writes carries: the encoder's lookahead.
pub fn pre_skip() -> u16 {
    // The lookahead does not depend on the configuration.
    crossfade_opus::Encoder::new(&EncoderConfig::default()).map_or(312, |e| e.lookahead() as u16)
}

/// Bits per second for the whole stream when the job names none: 96 kb/s for mono, 128 kb/s
/// for stereo.
pub fn default_bitrate(channels: u8) -> u32 {
    if channels == 1 { 96_000 } else { 128_000 }
}

/// The `OpusHead` (RFC 7845 5.1) of a mono or stereo stream (mapping family 0): version 1, the
/// pre-skip, `input_sample_rate` (informational) and a gain of 0.
pub fn head(channels: u8, input_sample_rate: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(19);
    h.extend_from_slice(b"OpusHead");
    h.push(1);
    h.push(channels);
    h.extend_from_slice(&pre_skip().to_le_bytes());
    h.extend_from_slice(&input_sample_rate.to_le_bytes());
    h.extend_from_slice(&0i16.to_le_bytes());
    h.push(0);
    h
}

struct OpusEncoder {
    inner: crossfade_opus::Encoder,
    channels: usize,
    frame_size: usize,
    /// Samples taken in and not coded yet.
    queued: Vec<f32>,
    /// Frames taken in, over the whole stream.
    total: u64,
    next_pts: i64,
    scratch: Vec<u8>,
    pending: VecDeque<Packet>,
    drained: bool,
    failed: Option<Error>,
    extradata: Vec<u8>,
}

pub fn encoder(spec: &AudioSpec, options: &EncodeOptions) -> Result<Box<dyn Encoder>, Error> {
    if spec.format() != SampleFormat::F32 {
        return Err(unsupported(format!(
            "opus encodes f32 samples, not {}",
            spec.format().name()
        )));
    }
    if spec.sample_rate() != SAMPLE_RATE {
        return Err(unsupported(format!(
            "opus is coded at 48000 Hz only, not {} Hz",
            spec.sample_rate()
        )));
    }
    let channels = match spec.layout().channels() {
        c @ (1 | 2) => c as u8,
        c => {
            return Err(unsupported(format!(
                "surround opus is not written yet ({c} channels); use --channels stereo or mono"
            )));
        }
    };
    let mut config = EncoderConfig::default();
    config.channels = channels;
    config.bitrate = options.bitrate.unwrap_or_else(|| default_bitrate(channels));
    config.vbr = !options.constant_bitrate;
    let inner = crossfade_opus::Encoder::new(&config).map_err(map)?;
    Ok(Box::new(OpusEncoder {
        channels: usize::from(channels),
        frame_size: inner.frame_size(),
        inner,
        queued: Vec::new(),
        total: 0,
        next_pts: -i64::from(pre_skip()),
        scratch: Vec::new(),
        pending: VecDeque::new(),
        drained: false,
        failed: None,
        extradata: head(channels, options.input_sample_rate.unwrap_or(SAMPLE_RATE)),
    }))
}

impl OpusEncoder {
    /// Codes one packet from the front of the queue (padded with silence if it is short) and
    /// states `duration` for it.
    fn code(&mut self, duration: u64) -> Result<(), Error> {
        let whole = self.frame_size * self.channels;
        self.queued.resize(self.queued.len().max(whole), 0.0);
        self.inner
            .encode(&self.queued[..whole], &mut self.scratch)
            .map_err(map)?;
        self.queued.drain(..whole);
        self.pending.push_back(Packet {
            stream: 0,
            pts: self.next_pts,
            duration,
            data: self.scratch.clone(),
        });
        self.next_pts += self.frame_size as i64;
        Ok(())
    }
}

impl Encoder for OpusEncoder {
    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        let Samples::F32(v) = &frame.samples else {
            return Err(unsupported(format!(
                "opus encodes f32 samples, not {}",
                frame.samples.format().name()
            )));
        };
        if frame.sample_rate != SAMPLE_RATE || usize::from(frame.layout.channels()) != self.channels
        {
            return Err(unsupported(format!(
                "frame is {} Hz x {} ch but the opus encoder is 48000 Hz x {} ch",
                frame.sample_rate,
                frame.layout.channels(),
                self.channels
            )));
        }
        if self.drained {
            return Err(unsupported("send_frame after drain"));
        }
        if !v.len().is_multiple_of(self.channels) {
            return Err(Error::Malformed {
                what: format!(
                    "{} samples is not a whole number of {}-channel frames",
                    v.len(),
                    self.channels
                ),
            });
        }
        self.queued.extend_from_slice(v);
        self.total += (v.len() / self.channels) as u64;
        // Packets coded here end before the input does: only `drain` ever cuts one short.
        while self.queued.len() >= self.frame_size * self.channels {
            self.code(self.frame_size as u64)?;
        }
        Ok(())
    }

    fn receive_packet(&mut self, out: &mut Packet) -> Result<bool, Error> {
        if let Some(e) = self.failed.take() {
            return Err(e);
        }
        match self.pending.pop_front() {
            Some(p) => {
                *out = p;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn drain(&mut self) {
        if self.drained {
            return;
        }
        self.drained = true;
        // A failure here cannot be returned; the next `receive_packet` reports it.
        while self.total > 0 && self.next_pts < self.total as i64 {
            let left = self.total as i64 - self.next_pts;
            let duration = left.min(self.frame_size as i64) as u64;
            if let Err(e) = self.code(duration) {
                self.failed = Some(e);
                return;
            }
        }
    }

    fn extradata(&self) -> &[u8] {
        &self.extradata
    }
}

#[cfg(test)]
#[path = "opus_encode_tests.rs"]
mod tests;
