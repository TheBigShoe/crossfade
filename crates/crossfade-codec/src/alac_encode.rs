//! ALAC encoder adapter: the inverse of the decoder adapter in `alac`. Frames come in left-
//! justified in their container and in WAVE channel order; `crossfade_alac::Encoder` takes
//! them right-justified at the coded depth in ALAC's order, 4096 frames to a packet.
//!
//! The coded depth is 16, 20, 24 or 32 ([`coded_depth`]); a true depth ALAC has no name for
//! is coded at the next one up (12 real bits in `S16` are written as 16-bit). The encoder
//! keeps state from packet to packet, so it runs on the calling thread whatever it is offered.

use crate::EncodeOptions;
use crate::alac::engine_error as map;
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, Encoder, Error, Packet, SampleFormat, Samples,
};
use std::collections::VecDeque;

fn unsupported(what: impl Into<String>) -> Error {
    Error::Unsupported { what: what.into() }
}

/// The depth ALAC codes `format`'s samples at, given their true depth (`None`: the format's
/// full width), or `None` when ALAC cannot hold the format.
pub fn coded_depth(format: SampleFormat, true_depth: Option<u8>) -> Option<u8> {
    match format {
        SampleFormat::S16 => Some(16),
        SampleFormat::S24 => Some(if true_depth.is_some_and(|d| d <= 20) {
            20
        } else {
            24
        }),
        SampleFormat::S32 => Some(32),
        SampleFormat::U8 | SampleFormat::F32 | SampleFormat::F64 => None,
    }
}

/// The one layout ALAC has for `channels` channels (1 to 8), in WAVE order.
pub fn layout_for(channels: u16) -> Option<ChannelLayout> {
    let wave = u8::try_from(channels)
        .ok()
        .and_then(crossfade_alac::wave_layout)?;
    ChannelLayout::from_mask(wave.mask, channels)
}

/// What is wrong with writing `layout` as ALAC, if anything: the layout it has for that many
/// channels is the only one it stores (a count past 8 is the encoder's to refuse).
pub fn layout_problem(layout: &ChannelLayout) -> Option<String> {
    let wanted = layout_for(layout.channels()).filter(|w| w != layout)?;
    let words = |l: &ChannelLayout| {
        l.name().map_or_else(
            || {
                if l.positions().is_none() {
                    format!("{} unlabelled channels", l.channels())
                } else {
                    format!("{} channels in another layout", l.channels())
                }
            },
            str::to_owned,
        )
    };
    Some(format!(
        "alac stores {} channels in one layout only ({}) and this stream is {}",
        layout.channels(),
        wanted.name().map_or_else(
            || format!("its own {}-channel order", layout.channels()),
            str::to_owned
        ),
        words(layout)
    ))
}

struct AlacEncoder {
    inner: crossfade_alac::Encoder,
    rate: u32,
    format: SampleFormat,
    channels: usize,
    /// For each ALAC channel, the WAVE-order channel that feeds it.
    to_alac: Vec<usize>,
    /// Container width minus the coded depth: how far samples shift right.
    shift: u32,
    frame_length: usize,
    /// Samples taken in and not coded yet: ALAC order, right-justified.
    queued: Vec<i32>,
    scratch: Vec<u8>,
    emitted: u64,
    pending: VecDeque<Packet>,
    drained: bool,
    failed: Option<Error>,
    extradata: Vec<u8>,
}

pub fn encoder(spec: &AudioSpec, options: &EncodeOptions) -> Result<Box<dyn Encoder>, Error> {
    let format = spec.format();
    let Some(depth) = coded_depth(format, options.bits_per_sample) else {
        let why = if format.is_float() {
            "alac cannot store floating-point samples".to_owned()
        } else {
            format!("alac cannot code {}-bit samples", format.bits())
        };
        return Err(unsupported(format!(
            "{why}; choose --sample-format s16, s24 or s32"
        )));
    };
    let layout = spec.layout();
    let count = layout.channels();
    let channels = u8::try_from(count)
        .ok()
        .filter(|n| (1..=8).contains(n))
        .ok_or_else(|| unsupported(format!("alac holds at most 8 channels, got {count}")))?;
    if let Some(problem) = layout_problem(layout) {
        return Err(unsupported(problem));
    }
    let wave = crossfade_alac::wave_layout(channels)
        .ok_or_else(|| unsupported("alac has no layout for this channel count"))?;
    let mut to_alac = vec![0; usize::from(channels)];
    for (wave_channel, &alac_channel) in wave.from_alac.iter().enumerate() {
        to_alac[usize::from(alac_channel)] = wave_channel;
    }
    let inner = crossfade_alac::Encoder::new(crossfade_alac::EncoderConfig::new(
        spec.sample_rate(),
        channels,
        depth,
    ))
    .map_err(map)?;
    let extradata = inner.config().write().to_vec();
    Ok(Box::new(AlacEncoder {
        inner,
        rate: spec.sample_rate(),
        format,
        channels: usize::from(channels),
        to_alac,
        shift: format.bits() as u32 - u32::from(depth),
        frame_length: crossfade_alac::DEFAULT_FRAME_LENGTH as usize,
        queued: Vec::new(),
        scratch: Vec::new(),
        emitted: 0,
        pending: VecDeque::new(),
        drained: false,
        failed: None,
        extradata,
    }))
}

impl AlacEncoder {
    /// Codes every whole packet queued, and with `all` the short one that is left.
    fn code(&mut self, all: bool) -> Result<(), Error> {
        let whole = self.frame_length * self.channels;
        let mut at = 0;
        while at < self.queued.len() && (all || self.queued.len() - at >= whole) {
            let end = (at + whole).min(self.queued.len());
            self.inner
                .encode(&self.queued[at..end], &mut self.scratch)
                .map_err(map)?;
            let duration = ((end - at) / self.channels) as u64;
            self.pending.push_back(Packet {
                stream: 0,
                pts: self.emitted as i64,
                duration,
                data: self.scratch.clone(),
            });
            self.emitted += duration;
            at = end;
        }
        self.queued.drain(..at);
        Ok(())
    }

    /// Appends `v` (WAVE order) in ALAC's order, `shift` bits down.
    fn queue<T: Copy + Into<i32>>(&mut self, v: &[T]) {
        self.queued.reserve(v.len());
        for frame in v.chunks_exact(self.channels) {
            let shift = self.shift;
            self.queued
                .extend(self.to_alac.iter().map(|&w| frame[w].into() >> shift));
        }
    }
}

impl Encoder for AlacEncoder {
    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        if frame.samples.format() != self.format
            || frame.sample_rate != self.rate
            || usize::from(frame.layout.channels()) != self.channels
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
        if !frame.samples.len().is_multiple_of(self.channels) {
            return Err(Error::Malformed {
                what: format!(
                    "{} samples is not a whole number of {}-channel frames",
                    frame.samples.len(),
                    self.channels
                ),
            });
        }
        match &frame.samples {
            Samples::S16(v) => self.queue(v),
            Samples::S32(v) => self.queue(v),
            Samples::S24(v) => {
                if v.iter().any(|&x| !(-(1 << 23)..1 << 23).contains(&x)) {
                    return Err(Error::Malformed {
                        what: "24-bit sample outside -2^23..2^23-1".into(),
                    });
                }
                self.queue(v);
            }
            Samples::U8(_) | Samples::F32(_) | Samples::F64(_) => {
                unreachable!("the constructor refused these formats")
            }
        }
        self.code(false)
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
        match self.code(true) {
            Ok(()) => self.extradata = self.inner.config().write().to_vec(),
            Err(e) => self.failed = Some(e),
        }
    }

    fn extradata(&self) -> &[u8] {
        &self.extradata
    }
}

#[cfg(test)]
#[path = "alac_encode_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "alac_encode_flow_tests.rs"]
mod flow_tests;
