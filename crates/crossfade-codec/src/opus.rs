//! Opus: the engine-facing adapter over `crossfade-opus`. The stream's `extradata` is its
//! `OpusHead` (RFC 7845 5.1); every stream, mono and stereo included, decodes through a
//! `MultistreamDecoder` (one stream for mapping family 0), with the header's output gain
//! applied, as RFC 7845 requires. One packet in, one frame out, decoded at once and stamped
//! with the packet's pts; channels come out in the stream layout's order (family 1 is Vorbis's
//! order, reordered as the Vorbis adapter does).
//!
//! A packet the decoder refuses (invalid framing, a corrupt frame) is concealed for the
//! duration its TOC gives, not skipped as Vorbis's are: Opus has concealment, so the timeline
//! keeps no hole and the audio fades instead of jumping. A packet without even a TOC (empty)
//! has no duration and yields nothing.

use crate::vorbis::speakers;
use crossfade_core::{AudioFrame, Decoder, Error, Packet, SampleFormat, Samples, StreamInfo};
use crossfade_opus::{MultistreamDecoder, OpusHead};

fn engine_error(e: crossfade_opus::Error) -> Error {
    match e {
        crossfade_opus::Error::Unsupported(_) => Error::Unsupported {
            what: e.to_string(),
        },
        other => Error::Malformed {
            what: other.to_string(),
        },
    }
}

struct OpusDecoder {
    inner: MultistreamDecoder,
    template: AudioFrame,
    ready: Option<AudioFrame>,
    spare: Option<AudioFrame>,
    /// Decoded samples in the codec's channel order, before reordering.
    decoded: Vec<f32>,
    /// For each output channel (in the layout's order), the decoded channel it takes; `None`
    /// when they are the same.
    order: Option<Vec<usize>>,
}

/// A decoder for an Opus stream whose `extradata` is its `OpusHead`. The stream must be 48 kHz
/// `F32` with the header's channel count; with a positioned layout of more than two channels
/// it must be [`ChannelLayout::vorbis_order`]'s (family 1), and the channels come out in it.
///
/// [`ChannelLayout::vorbis_order`]: crossfade_core::ChannelLayout::vorbis_order
pub fn decoder(stream: &StreamInfo) -> Result<Box<dyn Decoder>, Error> {
    let head = OpusHead::parse(&stream.extradata).map_err(engine_error)?;
    let audio = &stream.audio;
    if audio.sample_rate() != 48_000
        || audio.layout().channels() != u16::from(head.channels)
        || audio.format() != SampleFormat::F32
    {
        return Err(Error::Malformed {
            what: "opus: stream description disagrees with its identification header".into(),
        });
    }
    let mut inner =
        MultistreamDecoder::new(&head.mapping, usize::from(head.channels)).map_err(engine_error)?;
    inner.set_gain(head.output_gain);
    let order = match audio.layout().positions() {
        Some(positions) if head.channels > 2 => {
            let codec = speakers(head.channels).unwrap_or_default();
            let order: Option<Vec<usize>> = positions
                .iter()
                .map(|p| codec.iter().position(|c| c == p))
                .collect();
            match order {
                Some(o) if o.len() == codec.len() => Some(o),
                _ => {
                    return Err(Error::Malformed {
                        what: format!(
                            "opus: the stream's layout {:?} is not Vorbis's for {} channels",
                            audio.layout(),
                            head.channels
                        ),
                    });
                }
            }
        }
        _ => None,
    };
    Ok(Box::new(OpusDecoder {
        inner,
        template: AudioFrame::new(audio),
        ready: None,
        spare: None,
        decoded: Vec::new(),
        order,
    }))
}

impl Decoder for OpusDecoder {
    fn send_packet(&mut self, packet: &Packet) -> Result<(), Error> {
        let mut frame = self.spare.take().unwrap_or_else(|| self.template.clone());
        frame.sample_rate = self.template.sample_rate;
        if frame.layout != self.template.layout {
            frame.layout = self.template.layout.clone();
        }
        let Samples::F32(out) = &mut frame.samples else {
            unreachable!("only F32 frames are kept");
        };
        out.clear();
        let target = if self.order.is_some() {
            self.decoded.clear();
            &mut self.decoded
        } else {
            &mut *out
        };
        let n = match self.inner.decode(Some(&packet.data), target) {
            Ok(n) => n,
            // See the module's doc: concealed for its duration, or nothing without a TOC.
            Err(_) => match crossfade_opus::packet_duration(&packet.data) {
                Ok(d) => self.inner.conceal(d, target).map_err(engine_error)?,
                Err(_) => 0,
            },
        };
        if let Some(order) = &self.order {
            let ch = order.len();
            out.resize(n * ch, 0.0);
            for (o, d) in out.chunks_exact_mut(ch).zip(self.decoded.chunks_exact(ch)) {
                for (x, &k) in o.iter_mut().zip(order) {
                    *x = d[k];
                }
            }
        }
        if n > 0 {
            frame.pts = packet.pts;
            self.ready = Some(frame);
        } else {
            self.spare = Some(frame);
        }
        Ok(())
    }

    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        match self.ready.take() {
            Some(mut frame) => {
                std::mem::swap(out, &mut frame);
                if frame.samples.format() == SampleFormat::F32 {
                    self.spare = Some(frame);
                }
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Nothing is held back: every packet is decoded when it is sent.
    fn drain(&mut self) {}

    fn reset(&mut self) {
        self.inner.reset();
        if let Some(f) = self.ready.take() {
            self.spare = Some(f);
        }
    }
}

#[cfg(test)]
#[path = "opus_tests.rs"]
mod tests;
