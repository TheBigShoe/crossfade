//! Little-endian PCM: bytes to samples and back, one sample format per codec.

use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Decoder, Encoder, Error, Packet, SampleFormat,
    Samples, StreamInfo,
};

fn pcm_format(codec: CodecId) -> Result<SampleFormat, Error> {
    // The byte order is fixed little-endian below; refuse the rest instead of misreading it.
    if codec.is_pcm()
        && CodecId::pcm_for(codec.pcm_sample_format().unwrap_or(SampleFormat::U8)) != codec
    {
        return Err(Error::Unsupported {
            what: format!("{} is not built yet", codec.name()),
        });
    }
    codec.pcm_sample_format().ok_or_else(|| Error::Unsupported {
        what: format!("{} is not a PCM codec", codec.name()),
    })
}

/// Sign-extends three little-endian bytes into an `i32`.
fn s24_from_le(b: &[u8; 3]) -> i32 {
    i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8
}

fn decode_into(data: &[u8], out: &mut Samples) {
    // Each arm is a tight loop over fixed-size chunks; no per-sample format
    // branch. Callers guarantee whole samples, so the remainder is empty.
    match out {
        Samples::U8(v) => v.extend_from_slice(data),
        Samples::S16(v) => v.extend(
            data.as_chunks::<2>()
                .0
                .iter()
                .map(|c| i16::from_le_bytes(*c)),
        ),
        Samples::S24(v) => v.extend(data.as_chunks::<3>().0.iter().map(s24_from_le)),
        Samples::S32(v) => v.extend(
            data.as_chunks::<4>()
                .0
                .iter()
                .map(|c| i32::from_le_bytes(*c)),
        ),
        Samples::F32(v) => v.extend(
            data.as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c)),
        ),
        Samples::F64(v) => v.extend(
            data.as_chunks::<8>()
                .0
                .iter()
                .map(|c| f64::from_le_bytes(*c)),
        ),
    }
}

fn encode_into(samples: &Samples, out: &mut Vec<u8>) {
    match samples {
        Samples::U8(v) => out.extend_from_slice(v),
        Samples::S16(v) => out.extend(v.iter().flat_map(|x| x.to_le_bytes())),
        // Low three bytes; callers keep values within +-2^23.
        Samples::S24(v) => out.extend(v.iter().flat_map(|x| {
            let b = x.to_le_bytes();
            [b[0], b[1], b[2]]
        })),
        Samples::S32(v) => out.extend(v.iter().flat_map(|x| x.to_le_bytes())),
        Samples::F32(v) => out.extend(v.iter().flat_map(|x| x.to_le_bytes())),
        Samples::F64(v) => out.extend(v.iter().flat_map(|x| x.to_le_bytes())),
    }
}

#[derive(Debug)]
pub struct PcmDecoder {
    sample_rate: u32,
    layout: ChannelLayout,
    format: SampleFormat,
    bytes_per_frame: usize,
    pending: AudioFrame,
    has_pending: bool,
}

impl PcmDecoder {
    pub fn new(stream: &StreamInfo) -> Result<Self, Error> {
        let format = pcm_format(stream.codec)?;
        let layout = stream.audio.layout().clone();
        let spec = AudioSpec::new(stream.audio.sample_rate(), format, layout.clone())?;
        Ok(Self {
            sample_rate: spec.sample_rate(),
            format,
            bytes_per_frame: spec.bytes_per_frame(),
            layout,
            pending: AudioFrame::new(&spec),
            has_pending: false,
        })
    }
}

impl Decoder for PcmDecoder {
    fn send_packet(&mut self, packet: &Packet) -> Result<(), Error> {
        if self.has_pending {
            return Err(Error::Unsupported {
                what: "send_packet before the previous frame was received".into(),
            });
        }
        // bytes_per_frame is never zero: channels >= 1 and samples >= 1 byte.
        if !packet.data.len().is_multiple_of(self.bytes_per_frame) {
            return Err(Error::Malformed {
                what: format!(
                    "packet of {} bytes is not a whole number of {}-byte frames",
                    packet.data.len(),
                    self.bytes_per_frame
                ),
            });
        }
        // The swap in receive_frame may have brought in the caller's buffer.
        if self.pending.samples.format() == self.format {
            self.pending.samples.clear();
        } else {
            self.pending.samples = Samples::new(self.format);
        }
        decode_into(&packet.data, &mut self.pending.samples);
        self.pending.pts = packet.pts;
        self.has_pending = true;
        Ok(())
    }

    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        if !self.has_pending {
            return Ok(false);
        }
        self.has_pending = false;
        self.pending.sample_rate = self.sample_rate;
        self.pending.layout.clone_from(&self.layout);
        // Swap so the caller's old buffer becomes our next scratch.
        std::mem::swap(out, &mut self.pending);
        Ok(true)
    }

    fn drain(&mut self) {}
}

#[derive(Debug)]
pub struct PcmEncoder {
    format: SampleFormat,
    sample_rate: u32,
    channels: u16,
    pending: Packet,
    has_pending: bool,
}

impl PcmEncoder {
    /// The codec decides the sample format; `spec` supplies rate and channels.
    pub fn new(codec: CodecId, spec: &AudioSpec) -> Result<Self, Error> {
        Ok(Self {
            format: pcm_format(codec)?,
            sample_rate: spec.sample_rate(),
            channels: spec.layout().channels(),
            pending: Packet::default(),
            has_pending: false,
        })
    }
}

impl Encoder for PcmEncoder {
    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        if self.has_pending {
            return Err(Error::Unsupported {
                what: "send_frame before the previous packet was received".into(),
            });
        }
        if frame.samples.format() != self.format {
            return Err(Error::Unsupported {
                what: format!(
                    "frame is {} but the codec takes {}; convert it first",
                    frame.samples.format().name(),
                    self.format.name()
                ),
            });
        }
        if frame.sample_rate != self.sample_rate || frame.layout.channels() != self.channels {
            return Err(Error::Unsupported {
                what: format!(
                    "frame is {} Hz x {} ch but the encoder is {} Hz x {} ch",
                    frame.sample_rate,
                    frame.layout.channels(),
                    self.sample_rate,
                    self.channels
                ),
            });
        }
        let channels = usize::from(self.channels);
        if !frame.samples.len().is_multiple_of(channels) {
            return Err(Error::Malformed {
                what: format!(
                    "{} samples is not a whole number of {channels}-channel frames",
                    frame.samples.len()
                ),
            });
        }
        self.pending.data.clear();
        encode_into(&frame.samples, &mut self.pending.data);
        self.pending.stream = 0;
        self.pending.pts = frame.pts;
        self.pending.duration = (frame.samples.len() / channels) as u64;
        self.has_pending = true;
        Ok(())
    }

    fn receive_packet(&mut self, out: &mut Packet) -> Result<bool, Error> {
        if !self.has_pending {
            return Ok(false);
        }
        self.has_pending = false;
        std::mem::swap(out, &mut self.pending);
        Ok(true)
    }

    fn drain(&mut self) {}
}
