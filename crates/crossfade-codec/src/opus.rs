//! Opus: the engine-facing adapters over `crossfade-opus`. Decoding: the stream's `extradata` is its
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
use crossfade_core::{
    AudioFrame, AudioSpec, Decoder, Encoder, Error, Packet, SampleFormat, Samples, StreamInfo,
};
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

/// Samples per packet: 20 ms.
const FRAME: usize = 960;

/// The layout a stream encoded from `layout` reads back with: `layout` itself; for 3 to 8
/// unlabelled channels (a plain multichannel WAV) the layout Opus has for that count, whose
/// order they are taken to be in; and for side surrounds with no back ones (quad-side, 5.0,
/// 5.1) the same with back surrounds, Opus's one rear pair.
pub fn stream_layout(layout: &crossfade_core::ChannelLayout) -> crossfade_core::ChannelLayout {
    use crossfade_core::{Channel, ChannelLayout};
    let Ok(n) = u8::try_from(layout.channels()) else {
        return layout.clone();
    };
    let Ok(vorbis) = ChannelLayout::vorbis_order(n) else {
        return layout.clone();
    };
    match layout.positions() {
        None if (3..=8).contains(&n) => vorbis,
        Some(p) if !p.contains(&Channel::BackLeft) && !p.contains(&Channel::BackRight) => {
            let rear = |c: &Channel| match c {
                Channel::SideLeft => Channel::BackLeft,
                Channel::SideRight => Channel::BackRight,
                other => *other,
            };
            let moved: Vec<Channel> = p.iter().map(rear).collect();
            // The WAVE order of the two pairs is the same, so the channels stay in place.
            if vorbis.positions() == Some(moved.as_slice()) {
                vorbis
            } else {
                layout.clone()
            }
        }
        _ => layout.clone(),
    }
}

/// For each channel Opus codes (Vorbis order), the channel of `layout` it takes; `None` when
/// they are the same. An error names why the layout cannot be coded: more than two channels
/// must be one of the layouts Vorbis orders.
fn coded_order(layout: &crossfade_core::ChannelLayout) -> Result<Option<Vec<usize>>, Error> {
    let channels = layout.channels();
    if channels <= 2 {
        return Ok(None);
    }
    let written = stream_layout(layout);
    let order = u8::try_from(channels)
        .ok()
        .and_then(speakers)
        .and_then(|codec| {
            let positions = written.positions()?;
            let order: Option<Vec<usize>> = codec
                .iter()
                .map(|c| positions.iter().position(|p| p == c))
                .collect();
            order.filter(|o| o.len() == positions.len())
        });
    match order {
        Some(o) => Ok(Some(o)),
        None => {
            let what = match layout.name() {
                Some(name) => format!("the layout {name}"),
                None => format!("{channels} unlabelled channels"),
            };
            Err(Error::Unsupported {
                what: format!(
                    "opus cannot hold {what} (opus takes mono, stereo, 3.0, quad, 5.0, 5.1, 6.1 or \
                     7.1; choose --channels)"
                ),
            })
        }
    }
}

/// Why `spec` cannot be encoded as Opus, if it cannot: the encoder takes 48 kHz `F32` in
/// mono, stereo or a layout Vorbis orders.
pub fn check_encodable(spec: &AudioSpec) -> Result<(), Error> {
    let refuse = |what: String| Err(Error::Unsupported { what });
    coded_order(spec.layout())?;
    if spec.sample_rate() != 48_000 {
        return refuse(format!(
            "opus is encoded at 48000 Hz, not {} Hz",
            spec.sample_rate()
        ));
    }
    if spec.format() != SampleFormat::F32 {
        return refuse(format!(
            "opus is encoded from f32 samples, not {}",
            spec.format().name()
        ));
    }
    Ok(())
}

/// The `OpusHead` of the stream [`encoder`] writes for `spec`: known before any audio, since a
/// muxer writes it first.
pub fn head(spec: &AudioSpec) -> Result<Vec<u8>, Error> {
    check_encodable(spec)?;
    let enc = crossfade_opus::MultistreamEncoder::new(usize::from(spec.layout().channels()))
        .map_err(engine_error)?;
    Ok(enc.head(spec.sample_rate()).to_bytes())
}

/// Frames of any size in, 20 ms packets out. The first packet's pts is the first frame's less
/// the encoder's look-ahead (the header's pre-skip): the priming the timeline rule asks for.
/// After `drain` the input is padded with silence until every sample has come out of a
/// decoder.
struct OpusEncoder {
    inner: crossfade_opus::MultistreamEncoder,
    channels: usize,
    /// For each coded channel, the input channel it takes; `None` when they are the same.
    order: Option<Vec<usize>>,
    head: Vec<u8>,
    /// Samples not yet encoded, from `at` on.
    pending: Vec<f32>,
    at: usize,
    /// The pts of the next packet, once the first frame has set it.
    next_pts: Option<i64>,
    /// Frames taken in and frames the packets so far carry.
    fed: u64,
    coded: u64,
    draining: bool,
}

/// Why `bitrate` (bits per second) is not one Opus takes, if it is not.
pub fn check_bitrate(bitrate: u32) -> Result<(), Error> {
    if (6_000..=510_000).contains(&bitrate) {
        Ok(())
    } else {
        Err(Error::Unsupported {
            what: format!("bitrate {bitrate} (opus takes 6000 to 510000 bits per second)"),
        })
    }
}

/// An Opus encoder for `spec` (see [`check_encodable`]): CELT-only, one stream per channel
/// pair or channel above stereo, at a variable bitrate
/// around `bitrate` bits per second, or 64 kb/s a channel.
pub fn encoder(spec: &AudioSpec, bitrate: Option<u32>) -> Result<Box<dyn Encoder>, Error> {
    let head = head(spec)?;
    let channels = usize::from(spec.layout().channels());
    let order = coded_order(spec.layout())?;
    let mut inner = crossfade_opus::MultistreamEncoder::new(channels).map_err(engine_error)?;
    inner.set_vbr(true);
    if let Some(b) = bitrate {
        check_bitrate(b)?;
        // Each stream has the format's range to itself.
        inner.set_bitrate(b).map_err(|_| Error::Unsupported {
            what: format!("bitrate {b} is out of range for {channels} channels of opus"),
        })?;
    }
    Ok(Box::new(OpusEncoder {
        inner,
        channels,
        order,
        head,
        pending: Vec::new(),
        at: 0,
        next_pts: None,
        fed: 0,
        coded: 0,
        draining: false,
    }))
}

impl Encoder for OpusEncoder {
    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        let Samples::F32(samples) = &frame.samples else {
            return Err(Error::Unsupported {
                what: "opus is encoded from f32 samples".into(),
            });
        };
        if self.next_pts.is_none() {
            self.next_pts = Some(frame.pts - self.inner.lookahead() as i64);
        }
        // What was encoded is dropped before more is added, so the buffer stays small.
        self.pending.drain(..self.at);
        self.at = 0;
        match &self.order {
            None => self.pending.extend_from_slice(samples),
            Some(order) => {
                for frame in samples.chunks_exact(self.channels) {
                    self.pending.extend(order.iter().map(|&k| frame[k]));
                }
            }
        }
        self.fed += (samples.len() / self.channels) as u64;
        Ok(())
    }

    fn receive_packet(&mut self, out: &mut Packet) -> Result<bool, Error> {
        let need = FRAME * self.channels;
        let Some(pts) = self.next_pts else {
            return Ok(false);
        };
        if self.pending.len() - self.at < need {
            // The look-ahead still holds the input's end until that much more went in.
            let owed = self.fed + self.inner.lookahead() as u64;
            if !self.draining || self.coded >= owed {
                return Ok(false);
            }
            self.pending.resize(self.at + need, 0.0);
        }
        out.data.clear();
        self.inner
            .encode(&self.pending[self.at..self.at + need], &mut out.data)
            .map_err(engine_error)?;
        self.at += need;
        self.coded += FRAME as u64;
        out.stream = 0;
        out.pts = pts;
        out.duration = FRAME as u64;
        self.next_pts = Some(pts + FRAME as i64);
        Ok(true)
    }

    fn drain(&mut self) {
        self.draining = true;
    }

    fn extradata(&self) -> &[u8] {
        &self.head
    }
}
