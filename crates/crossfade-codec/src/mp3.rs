//! MP3 decoder adapter: one packet (one whole Layer III frame) in, one frame of interleaved
//! `F32` samples out, stamped with the packet's pts. The decoding lives in `crossfade-mp3`.
//!
//! Every packet yields its frame's worth of samples, so the timeline stays exact: a frame
//! whose main data starts before the first packet fed (at the start of a stream cut from a
//! longer one, or after `reset` for a seek) comes out as silence and is not an error. Any
//! other failure, and a reservoir failure once a frame has decoded, is an error.

use crossfade_core::{AudioFrame, Decoder, Error, Packet, SampleFormat, Samples, StreamInfo};

fn engine_error(e: crossfade_mp3::Error) -> Error {
    match e {
        crossfade_mp3::Error::UnsupportedLayer(_) => Error::Unsupported {
            what: e.to_string(),
        },
        other => Error::Malformed {
            what: other.to_string(),
        },
    }
}

struct Mp3Decoder {
    inner: crossfade_mp3::Decoder,
    template: AudioFrame,
    channels: u8,
    sample_rate: u32,
    /// A frame decoded since the start or the last reset: from then on a missing reservoir
    /// means damage.
    primed: bool,
    ready: Option<AudioFrame>,
    spare: Option<AudioFrame>,
}

pub fn decoder(stream: &StreamInfo) -> Result<Box<dyn Decoder>, Error> {
    if stream.audio.format() != SampleFormat::F32 {
        return Err(Error::Unsupported {
            what: "mp3: the decoder gives f32 samples only".into(),
        });
    }
    let channels = stream.audio.layout().channels();
    let Ok(channels @ (1 | 2)) = u8::try_from(channels) else {
        return Err(Error::Unsupported {
            what: format!("mp3: {channels} channels"),
        });
    };
    Ok(Box::new(Mp3Decoder {
        inner: crossfade_mp3::Decoder::new(),
        template: AudioFrame::new(&stream.audio),
        channels,
        sample_rate: stream.audio.sample_rate(),
        primed: false,
        ready: None,
        spare: None,
    }))
}

impl Decoder for Mp3Decoder {
    fn send_packet(&mut self, packet: &Packet) -> Result<(), Error> {
        // The spare is whatever the caller last handed in: make it this stream's shape.
        let mut frame = self.spare.take().unwrap_or_else(|| self.template.clone());
        frame.sample_rate = self.sample_rate;
        if frame.layout != self.template.layout {
            frame.layout = self.template.layout.clone();
        }
        if frame.samples.format() != SampleFormat::F32 {
            frame.samples = Samples::new(SampleFormat::F32);
        }
        let Samples::F32(pcm) = &mut frame.samples else {
            unreachable!("set to F32 above");
        };
        pcm.clear();
        match self.inner.decode_frame(&packet.data, pcm) {
            Ok(info) => {
                let h = info.header;
                if h.channels() != self.channels || h.sample_rate != self.sample_rate {
                    return Err(Error::Malformed {
                        what: format!(
                            "mp3: a frame of {} channels at {} Hz in a stream of {} at {} Hz",
                            h.channels(),
                            h.sample_rate,
                            self.channels,
                            self.sample_rate
                        ),
                    });
                }
                self.primed = true;
            }
            Err(e) if e.is_reservoir() && !self.primed => {}
            Err(e) => return Err(engine_error(e)),
        }
        frame.pts = packet.pts;
        self.ready = Some(frame);
        Ok(())
    }

    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        match self.ready.take() {
            Some(mut frame) => {
                std::mem::swap(out, &mut frame);
                self.spare = Some(frame);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn drain(&mut self) {}

    fn reset(&mut self) {
        self.inner.reset();
        self.primed = false;
        self.ready = None;
    }
}

#[cfg(test)]
#[path = "mp3_tests.rs"]
mod tests;
