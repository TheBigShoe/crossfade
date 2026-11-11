//! FLAC encoder: `crossfade_flac::Encoder` behind the streaming [`Encoder`] trait.
//!
//! Input is re-blocked into fixed-size blocks, each coded to one complete FLAC frame, so a
//! packet is exactly one frame. Everything runs on the calling thread: identical input gives
//! identical bytes anywhere. The compression levels (0 to 8, `None` means 5) and what each one
//! tries are the standalone encoder's; see its `encode` module.
//!
//! # Limits
//!
//! 8, 16, 24 and 32 bit integer samples, 1 to 8 channels, 1 to 1 048 575 Hz. The engine's
//! samples are left-justified in their container and carry no true bit depth, so a stream is
//! coded at the container depth (STREAMINFO says 8, 16, 24 or 32). A 20-bit source decoded
//! into 24-bit containers has four zero low bits in every sample; the encoder finds and drops
//! them, so the frames are as small as a 20-bit stream's, but STREAMINFO still says 24.

use crate::EncodeOptions;
use crate::flac::decode::engine_error as map;
use crossfade_core::{AudioFrame, AudioSpec, Encoder, Error, Packet, SampleFormat, Samples};
use crossfade_flac::streaminfo::StreamInfoBlock;
use crossfade_flac::{Encoder as FlacCoder, EncoderConfig};
use std::collections::VecDeque;

const DEFAULT_LEVEL: u8 = 5;
const MAX_LEVEL: u8 = 8;
const MAX_RATE: u32 = 1_048_575;
const MAX_CHANNELS: u16 = 8;

fn unsupported(what: impl Into<String>) -> Error {
    Error::Unsupported { what: what.into() }
}

fn bits_of(format: SampleFormat) -> Result<u8, Error> {
    match format {
        SampleFormat::U8 => Ok(8),
        SampleFormat::S16 => Ok(16),
        SampleFormat::S24 => Ok(24),
        SampleFormat::S32 => Ok(32),
        SampleFormat::F32 | SampleFormat::F64 => Err(unsupported(
            "flac encoding of float samples is not supported; convert to s16 or s24 first",
        )),
    }
}

struct FlacEncoder {
    inner: FlacCoder,
    rate: u32,
    channels: u16,
    format: SampleFormat,
    block: u64,
    /// Samples per channel taken in, and in frames already emitted.
    taken: u64,
    emitted: u64,
    /// Raw samples of the current `send_frame`, when they need converting.
    raw: Vec<i32>,
    /// Frames the inner encoder just wrote, and where each one ends.
    bytes: Vec<u8>,
    ends: Vec<usize>,
    pending: VecDeque<Packet>,
    drained: bool,
    failed: Option<Error>,
    extradata: [u8; StreamInfoBlock::LEN],
}

pub fn encoder(spec: &AudioSpec, options: &EncodeOptions) -> Result<Box<dyn Encoder>, Error> {
    let bits = bits_of(spec.format())?;
    let channels = spec.layout().channels();
    if channels > MAX_CHANNELS {
        return Err(unsupported(format!(
            "flac holds at most {MAX_CHANNELS} channels, got {channels}"
        )));
    }
    let rate = spec.sample_rate();
    if rate > MAX_RATE {
        return Err(unsupported(format!(
            "flac supports sample rates up to {MAX_RATE} Hz, got {rate}"
        )));
    }
    let level = options.compression_level.unwrap_or(DEFAULT_LEVEL);
    if level > MAX_LEVEL {
        return Err(unsupported(format!(
            "flac compression level runs from 0 to {MAX_LEVEL}, got {level}"
        )));
    }
    let inner = FlacCoder::new(EncoderConfig {
        sample_rate: rate,
        channels: channels as u8,
        bits_per_sample: bits,
        level,
        threads: 1,
    })
    .map_err(map)?;
    let block = inner.block_size();
    let extradata = inner.stream_info().write();
    Ok(Box::new(FlacEncoder {
        inner,
        rate,
        channels,
        format: spec.format(),
        block: block as u64,
        taken: 0,
        emitted: 0,
        raw: Vec::new(),
        bytes: Vec::with_capacity(block * usize::from(channels) * 4 + 64),
        ends: Vec::new(),
        pending: VecDeque::new(),
        drained: false,
        failed: None,
        extradata,
    }))
}

impl FlacEncoder {
    /// Cuts what the inner encoder wrote into packets, one per frame.
    fn collect(&mut self, last_len: Option<u64>) {
        let mut start = 0;
        for &end in &self.ends {
            let duration = last_len.unwrap_or(self.block);
            self.pending.push_back(Packet {
                stream: 0,
                pts: self.emitted as i64,
                duration,
                data: self.bytes[start..end].to_vec(),
            });
            self.emitted += duration;
            start = end;
        }
        self.bytes.clear();
        self.ends.clear();
    }

    fn write(&mut self, samples: &[i32]) -> Result<(), Error> {
        self.taken += (samples.len() / usize::from(self.channels)) as u64;
        self.inner
            .write_ends(samples, &mut self.bytes, &mut self.ends)
            .map_err(map)?;
        self.collect(None);
        Ok(())
    }

    fn finish(&mut self) -> Result<(), Error> {
        let rest = self.taken - self.emitted;
        self.inner
            .flush_ends(&mut self.bytes, &mut self.ends)
            .map_err(map)?;
        self.collect(Some(rest));
        self.extradata = self.inner.finish().write();
        Ok(())
    }
}

impl Encoder for FlacEncoder {
    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        if frame.samples.format() != self.format
            || frame.sample_rate != self.rate
            || frame.layout.channels() != self.channels
        {
            return Err(unsupported(format!(
                "frame is {} Hz x {} ch {} but the encoder is {} Hz x {} ch {}",
                frame.sample_rate,
                frame.layout.channels(),
                frame.samples.format().name(),
                self.rate,
                self.channels,
                self.format.name()
            )));
        }
        if self.drained {
            return Err(unsupported("send_frame after drain"));
        }
        if !frame
            .samples
            .len()
            .is_multiple_of(usize::from(self.channels))
        {
            return Err(Error::Malformed {
                what: format!(
                    "{} samples is not a whole number of {}-channel frames",
                    frame.samples.len(),
                    self.channels
                ),
            });
        }
        let mut raw = std::mem::take(&mut self.raw);
        raw.clear();
        let result = match &frame.samples {
            Samples::S32(v) => self.write(v),
            Samples::U8(v) => {
                raw.extend(v.iter().map(|&x| i32::from(x) - 128));
                self.write(&raw)
            }
            Samples::S16(v) => {
                raw.extend(v.iter().map(|&x| i32::from(x)));
                self.write(&raw)
            }
            Samples::S24(v) => {
                // A min/max fold vectorises; an early-exit `any` does not.
                let (min, max) = v
                    .iter()
                    .fold((i32::MAX, i32::MIN), |(a, b), &s| (a.min(s), b.max(s)));
                if min < -(1 << 23) || max >= 1 << 23 {
                    Err(Error::Malformed {
                        what: "24-bit sample outside -2^23..2^23-1".into(),
                    })
                } else {
                    self.write(v)
                }
            }
            Samples::F32(_) | Samples::F64(_) => {
                unreachable!("the constructor refused these formats")
            }
        };
        self.raw = raw;
        result
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
        if let Err(e) = self.finish() {
            self.failed = Some(e);
        }
    }

    fn extradata(&self) -> &[u8] {
        &self.extradata
    }
}

#[cfg(test)]
mod tests;
