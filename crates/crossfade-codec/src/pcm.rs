//! Little-endian PCM: bytes to samples and back, one sample format per codec.

use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Decoder, Error, Packet, SampleFormat, Samples,
    StreamInfo,
};

fn pcm_format(codec: CodecId) -> Result<SampleFormat, Error> {
    codec.pcm_sample_format().ok_or_else(|| Error::Unsupported {
        what: format!("{} is not a PCM codec", codec.name()),
    })
}

/// Sign-extends three little-endian bytes into an `i32`.
fn s24_from_le(b: &[u8]) -> i32 {
    i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8
}

fn decode_into(data: &[u8], out: &mut Samples) {
    // Each arm is a tight loop over exact chunks; no per-sample format branch.
    match out {
        Samples::U8(v) => v.extend_from_slice(data),
        Samples::S16(v) => v.extend(
            data.chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]])),
        ),
        Samples::S24(v) => v.extend(data.chunks_exact(3).map(s24_from_le)),
        Samples::S32(v) => v.extend(
            data.chunks_exact(4)
                .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]])),
        ),
        Samples::F32(v) => v.extend(
            data.chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])),
        ),
        Samples::F64(v) => v.extend(
            data.chunks_exact(8)
                .map(|c| f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]])),
        ),
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
        if packet.data.len() % self.bytes_per_frame != 0 {
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
