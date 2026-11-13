//! AAC decoder adapter over `crossfade-aac` (built only with the `aac` feature: AAC is
//! patent-encumbered). The AudioSpecificConfig is the stream's extradata, from MP4 or
//! synthesised by the ADTS reader. Each packet gives one frame of interleaved `F32`, stamped
//! with the packet's pts: 1024 frames per raw data block, and a packet as long as several
//! blocks (an ADTS frame that cannot be split) holds them back to back.

use crossfade_aac::asc::AudioSpecificConfig;
use crossfade_aac::decoder::FRAME_LEN;
use crossfade_core::{AudioFrame, Decoder, Error, Packet, Samples, StreamInfo};

fn engine_error(e: crossfade_aac::Error) -> Error {
    match e {
        crossfade_aac::Error::Unsupported(what) => Error::Unsupported { what },
        other => Error::Malformed {
            what: other.to_string(),
        },
    }
}

struct AacDecoder {
    inner: crossfade_aac::Decoder,
    template: AudioFrame,
    ready: Option<AudioFrame>,
    /// The caller's last frame, filled next so steady state allocates nothing.
    spare: Option<AudioFrame>,
    pcm: Vec<f32>,
}

pub(crate) fn decoder(stream: &StreamInfo) -> Result<Box<dyn Decoder>, Error> {
    let config = AudioSpecificConfig::parse(&stream.extradata).map_err(engine_error)?;
    let inner = crossfade_aac::Decoder::new(&config).map_err(engine_error)?;
    let channels = stream.audio.layout().channels();
    if usize::from(channels) != inner.channels() {
        return Err(Error::Malformed {
            what: format!(
                "aac: the stream has {channels} channels, its configuration {}",
                inner.channels()
            ),
        });
    }
    let mut template = AudioFrame::new(&stream.audio);
    template.samples = Samples::F32(Vec::new());
    Ok(Box::new(AacDecoder {
        inner,
        template,
        ready: None,
        spare: None,
        pcm: Vec::new(),
    }))
}

impl Decoder for AacDecoder {
    fn send_packet(&mut self, packet: &Packet) -> Result<(), Error> {
        let len = FRAME_LEN as u64;
        let blocks = match packet.duration {
            d if d > len && d % len == 0 && d <= 4 * len => (d / len) as usize,
            _ => 1,
        };
        self.inner
            .decode_blocks(&packet.data, blocks, &mut self.pcm)
            .map_err(engine_error)?;
        let mut frame = self.spare.take().unwrap_or_else(|| self.template.clone());
        frame.pts = packet.pts;
        if let Samples::F32(v) = &mut frame.samples {
            v.clear();
            v.extend_from_slice(&self.pcm);
        }
        self.ready = Some(frame);
        Ok(())
    }

    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        match self.ready.take() {
            Some(mut frame) => {
                std::mem::swap(out, &mut frame);
                // The caller's old frame becomes the next one to fill when its shape fits.
                if frame.layout == self.template.layout && matches!(frame.samples, Samples::F32(_))
                {
                    self.template.sample_rate = frame.sample_rate;
                }
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn drain(&mut self) {}

    fn reset(&mut self) {
        self.inner.reset();
        self.ready = None;
    }
}
