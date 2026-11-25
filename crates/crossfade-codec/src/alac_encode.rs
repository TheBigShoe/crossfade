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

fn layout_words(layout: &ChannelLayout) -> String {
    layout.name().map_or_else(
        || format!("{} channels in another order", layout.channels()),
        str::to_owned,
    )
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
    let wanted = crossfade_alac::wave_layout(channels)
        .and_then(|w| ChannelLayout::from_mask(w.mask, count))
        .ok_or_else(|| unsupported("alac has no layout for this channel count"))?;
    let wave = crossfade_alac::wave_layout(channels).expect("checked above");
    if *layout != wanted {
        let advice = match wanted.name() {
            Some(name) => format!("use --channels {name} to convert"),
            None => "no --channels name selects it".to_owned(),
        };
        return Err(unsupported(format!(
            "alac stores {channels} channels in one layout only ({}) and this stream is {}; {advice}",
            layout_words(&wanted),
            layout_words(layout)
        )));
    }
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
    let width = format.bits() as u32;
    let extradata = inner.config().write().to_vec();
    Ok(Box::new(AlacEncoder {
        inner,
        rate: spec.sample_rate(),
        format,
        channels: usize::from(channels),
        to_alac,
        shift: width - u32::from(depth),
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

impl Encoder for AlacEncoder {
    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        let _ = (
            self.rate,
            self.format,
            self.channels,
            &self.to_alac,
            self.shift,
            self.frame_length,
            &self.queued,
            &self.scratch,
            self.emitted,
            self.drained,
            frame,
        );
        Err(unsupported("alac frames are not coded yet"))
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
        self.drained = true;
        let _ = (&mut self.inner, Samples::S16(Vec::new()));
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
