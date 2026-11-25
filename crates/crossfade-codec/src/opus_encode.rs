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
//! Depth. As `opusenc`, the encoder is told the source's depth (`OPUS_SET_LSB_DEPTH`, 8 to 24):
//! an integer source's true depth, 24 for a float or lossy one
//! (`EncodeOptions::source_bits_per_sample`, [`source_bits_per_sample`]).
//!
//! Surround. Three to eight channels are written as mapping family 1 through
//! `crossfade_opus::MultistreamEncoder`, from the one layout the engine calls Vorbis's for their
//! count (`ChannelLayout::vorbis_order`, the layout the decoder adapter gives): the adapter
//! reorders the frames from that layout's storage order to Vorbis's channel order, the inverse of
//! the decoder adapter. The same speakers with the single surround pair named side, and
//! unlabelled channels, are coded as that layout, channel for channel (`ChannelLayout::fit_to`,
//! [`layout_note`]); any other layout is refused by [`layout_problem`]; one or two channels
//! take any layout.

use crate::EncodeOptions;
use crate::layout;
use crate::opus::engine_error as map;
use crate::vorbis::speakers;
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, Encoder, Error, Packet, SampleFormat, Samples,
    StreamInfo,
};
use crossfade_opus::{EncoderConfig, MultistreamEncoder};
use std::collections::VecDeque;

fn unsupported(what: impl Into<String>) -> Error {
    Error::Unsupported { what: what.into() }
}

/// The only rate Opus is coded at.
pub const SAMPLE_RATE: u32 = 48_000;

/// The most channels Opus's mapping family 1 holds.
pub const MAX_CHANNELS: u16 = 8;

/// Bits per second for the whole stream when the job names none: 96 kb/s for mono, 128 kb/s
/// for stereo, and for surround what `opusenc` gives (64 kb/s for each stream and 32 kb/s more
/// for each coupled pair): 160 kb/s for 3 channels, 192 for 4, 256 for 5, 320 for 5.1, 416 for
/// 6.1 and 7.1.
pub fn default_bitrate(channels: u8) -> u32 {
    let (streams, coupled) = match channels {
        1 => return 96_000,
        2 => return 128_000,
        3 => (2, 1),
        4 => (2, 2),
        5 => (3, 2),
        6 => (4, 2),
        _ => (5, 3),
    };
    64_000 * streams + 32_000 * coupled
}

/// The one layout Opus stores `channels` (3 to 8) channels in: Vorbis's, in storage order.
pub fn layout_for(channels: u16) -> Option<ChannelLayout> {
    u8::try_from(channels)
        .ok()
        .filter(|n| (3..=8).contains(n))
        .and_then(|n| ChannelLayout::vorbis_order(n).ok())
}

/// What is wrong with writing `layout` as Opus, if anything: more than eight channels, or a
/// layout other than Vorbis's for its count that is not the same speakers under the other
/// surround name or unlabelled (one and two channels take any layout).
pub fn layout_problem(layout: &ChannelLayout) -> Option<String> {
    let n = layout.channels();
    if n > MAX_CHANNELS {
        return Some(format!("opus holds at most 8 channels, got {n}"));
    }
    layout::problem("opus", layout, &layout_for(n)?)
}

/// What a front end should say when `layout` is written as Vorbis's by assumption.
pub fn layout_note(layout: &ChannelLayout) -> Option<String> {
    layout::note("opus", layout, &layout_for(layout.channels())?)
}

/// The depth `source`'s samples were quantised to, for the Opus encoder's input depth: an
/// integer source's true depth (`bits_per_raw_sample`, else its format's width), `None` for a
/// float or lossy one, whose samples sit on no grid.
pub fn source_bits_per_sample(source: &StreamInfo) -> Option<u8> {
    let format = source.audio.format();
    if source.codec.is_lossy() || format.is_float() {
        return None;
    }
    let width = u8::try_from(format.bits()).ok()?;
    Some(source.bits_per_raw_sample.unwrap_or(width))
}

/// The encoder for `channels` (1 to 8) at the default settings, from which the header and the
/// pre-skip are read.
fn config(channels: u8) -> EncoderConfig {
    let mut config = EncoderConfig::default();
    config.channels = channels;
    config.bitrate = default_bitrate(channels);
    config
}

/// The family of the header and the packets for `channels`.
fn family(channels: u8) -> u8 {
    u8::from(channels > 2)
}

/// The pre-skip every stream this adapter writes carries: the encoder's lookahead.
pub fn pre_skip() -> u16 {
    // The lookahead does not depend on the configuration.
    MultistreamEncoder::new(&config(2), 0).map_or(312, |e| e.lookahead() as u16)
}

/// The `OpusHead` (RFC 7845 5.1) of a stream of `channels` channels (1 to 8): version 1, the
/// pre-skip, `input_sample_rate` (informational) and a gain of 0, with mapping family 0 for mono
/// and stereo and family 1 (the streams, the coupled pairs and the channel table) past that.
pub fn head(channels: u8, input_sample_rate: u32) -> Vec<u8> {
    MultistreamEncoder::new(&config(channels), family(channels))
        .map(|e| e.head(input_sample_rate).to_bytes())
        .unwrap_or_default()
}

struct OpusEncoder {
    inner: MultistreamEncoder,
    channels: usize,
    /// For each Vorbis-order channel, the channel of the frames sent that feeds it; `None` for
    /// mono and stereo.
    order: Option<Vec<usize>>,
    frame: Vec<f32>,
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
    if let Some(problem) = layout_problem(spec.layout()) {
        return Err(unsupported(problem));
    }
    let channels = spec.layout().channels() as u8;
    let mut config = config(channels);
    config.bitrate = options.bitrate.unwrap_or(config.bitrate);
    config.vbr = !options.constant_bitrate;
    config.lsb_depth = options.source_bits_per_sample.map_or(24, |b| b.clamp(8, 24));
    let inner = MultistreamEncoder::new(&config, family(channels)).map_err(map)?;
    let order = (channels > 2)
        .then(|| {
            // A renamed or unlabelled layout is coded as Vorbis's, channel for channel.
            let own = layout_for(u16::from(channels))?;
            let positions = own.positions()?;
            speakers(channels)?
                .iter()
                .map(|c| positions.iter().position(|p| p == c))
                .collect::<Option<Vec<usize>>>()
        })
        .flatten();
    if channels > 2 && order.is_none() {
        return Err(unsupported("opus: no speaker order for this layout"));
    }
    Ok(Box::new(OpusEncoder {
        channels: usize::from(channels),
        frame_size: inner.frame_size(),
        order,
        frame: Vec::new(),
        queued: Vec::new(),
        total: 0,
        next_pts: -i64::from(pre_skip()),
        scratch: Vec::new(),
        pending: VecDeque::new(),
        drained: false,
        failed: None,
        extradata: inner
            .head(options.input_sample_rate.unwrap_or(SAMPLE_RATE))
            .to_bytes(),
        inner,
    }))
}

impl OpusEncoder {
    /// Codes one packet from the front of the queue (padded with silence if it is short) and
    /// states `duration` for it.
    fn code(&mut self, duration: u64) -> Result<(), Error> {
        let whole = self.frame_size * self.channels;
        self.queued.resize(self.queued.len().max(whole), 0.0);
        let pcm = match &self.order {
            None => &self.queued[..whole],
            Some(order) => {
                self.frame.clear();
                for f in self.queued[..whole].chunks_exact(self.channels) {
                    self.frame.extend(order.iter().map(|&o| f[o]));
                }
                &self.frame
            }
        };
        self.inner.encode(pcm, &mut self.scratch).map_err(map)?;
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
