//! Vorbis: the engine-facing adapter over `crossfade-vorbis`. One packet in, at most one frame
//! out, decoded at once and stamped with the packet's pts; the first packet (and the first
//! after a reset) yields nothing, so no frame.
//!
//! An audio packet the decoder cannot use (a mode that does not exist, a header among the
//! audio, an empty packet) yields no frame and is not an error, as libvorbis and every player
//! built on it skip it: Xiph's own `unused-mode-test.ogg` holds 34 such packets.

use crossfade_core::{
    AudioFrame, Channel, Decoder, Error, Limits, Packet, SampleFormat, Samples, StreamInfo,
};
use crossfade_vorbis::Speaker;

fn engine_error(e: crossfade_vorbis::Error) -> Error {
    match e {
        crossfade_vorbis::Error::OverBudget { requested, budget } => Error::AllocLimitExceeded {
            requested,
            max: budget,
        },
        other => Error::Malformed {
            what: other.to_string(),
        },
    }
}

fn channel(s: Speaker) -> Option<Channel> {
    Some(match s {
        Speaker::FrontLeft => Channel::FrontLeft,
        Speaker::FrontCenter => Channel::FrontCenter,
        Speaker::FrontRight => Channel::FrontRight,
        Speaker::RearLeft => Channel::BackLeft,
        Speaker::RearRight => Channel::BackRight,
        Speaker::SideLeft => Channel::SideLeft,
        Speaker::SideRight => Channel::SideRight,
        Speaker::RearCenter => Channel::BackCenter,
        Speaker::Lfe => Channel::Lfe,
        _ => return None,
    })
}

/// Vorbis's speakers for `channels`, as the engine names them, in Vorbis's order.
fn speakers(channels: u8) -> Option<Vec<Channel>> {
    crossfade_vorbis::speakers(channels)?
        .iter()
        .map(|&s| channel(s))
        .collect()
}

struct VorbisDecoder {
    inner: crossfade_vorbis::Decoder,
    template: AudioFrame,
    ready: Option<AudioFrame>,
    spare: Option<AudioFrame>,
    channels: usize,
}

/// A decoder for a Vorbis stream whose `extradata` holds its three headers in Xiph lacing
/// (see [`crossfade_vorbis::pack_headers`]). The stream must describe the identification
/// header (rate, channel count) as `F32`; its layout may be [`ChannelLayout::vorbis_order`]'s, in which case the
/// channels come out in that order, or unpositioned, in which case they come out in Vorbis's.
pub fn decoder(stream: &StreamInfo) -> Result<Box<dyn Decoder>, Error> {
    let [id, comment, setup] =
        crossfade_vorbis::unpack_headers(&stream.extradata).map_err(engine_error)?;
    let ident = crossfade_vorbis::Ident::parse(id).map_err(engine_error)?;
    let limits = Limits::default();
    crossfade_vorbis::Comments::parse(comment, limits.max_alloc_bytes).map_err(engine_error)?;
    let setup = crossfade_vorbis::Setup::parse(setup, &ident, limits.max_alloc_bytes)
        .map_err(engine_error)?;
    limits.check_alloc(crossfade_vorbis::Decoder::bytes_needed(&setup))?;
    let audio = &stream.audio;
    if audio.sample_rate() != ident.sample_rate
        || audio.layout().channels() != u16::from(ident.channels)
        || audio.format() != SampleFormat::F32
    {
        return Err(Error::Malformed {
            what: "vorbis: stream description disagrees with its identification header".into(),
        });
    }
    let mut inner = crossfade_vorbis::Decoder::new(&ident, setup).map_err(engine_error)?;
    if let Some(positions) = audio.layout().positions() {
        let vorbis = speakers(ident.channels).unwrap_or_default();
        let order: Option<Vec<usize>> = vorbis
            .iter()
            .map(|c| positions.iter().position(|p| p == c))
            .collect();
        if !order.is_some_and(|o| o.len() == positions.len() && inner.set_channel_order(&o)) {
            return Err(Error::Malformed {
                what: format!(
                    "vorbis: the stream's layout {:?} is not Vorbis's for {} channels",
                    audio.layout(),
                    ident.channels
                ),
            });
        }
    }
    Ok(Box::new(VorbisDecoder {
        inner,
        template: AudioFrame::new(audio),
        ready: None,
        spare: None,
        channels: usize::from(ident.channels),
    }))
}

impl Decoder for VorbisDecoder {
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
        match self.inner.decode(&packet.data, out) {
            Ok(n) if n > 0 => {
                debug_assert_eq!(out.len(), n * self.channels);
                frame.pts = packet.pts;
                self.ready = Some(frame);
            }
            // Nothing yet, or a packet to skip (see the module's doc).
            Ok(_) | Err(crossfade_vorbis::Error::BadPacket(_)) => self.spare = Some(frame),
            Err(e) => return Err(engine_error(e)),
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
#[path = "vorbis_tests.rs"]
mod tests;
