use crate::{AudioSpec, ChannelLayout, CodecId, SampleFormat, TimeBase};

/// One stream of a container.
///
/// The stream's timeline starts at pts 0, the first sample frame meant to be heard; see
/// [`Packet::pts`] for what comes before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    pub index: u32,
    pub codec: CodecId,
    pub audio: AudioSpec,
    /// Unit of `Packet::pts` and `Packet::duration`.
    pub time_base: TimeBase,
    /// Sample frames meant to be heard, when the container knows: what is left once the
    /// encoder's priming (negative pts) is dropped and its padding at the end is cut. The engine
    /// plays nothing at or past it.
    pub frames: Option<u64>,
    /// The codec's header bytes (FLAC: the 34-byte STREAMINFO body). Empty for PCM.
    pub extradata: Vec<u8>,
    /// Significant bits per sample when fewer than `audio.format()` holds (e.g. 20 in S24).
    /// Samples are left-justified in the format.
    pub bits_per_raw_sample: Option<u8>,
}

impl StreamInfo {
    pub fn new(index: u32, codec: CodecId, audio: AudioSpec, time_base: TimeBase) -> Self {
        Self {
            index,
            codec,
            audio,
            time_base,
            frames: None,
            extradata: Vec::new(),
            bits_per_raw_sample: None,
        }
    }
}

/// Reused across reads: a demuxer clears and refills `data`, so steady state
/// allocates nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Packet {
    pub stream: u32,
    /// The timeline position, in the stream's time base, of the first sample frame that
    /// decoding this packet yields. The timeline starts at 0 with the first frame meant to be
    /// heard, so an encoder's priming (delay) frames have NEGATIVE pts. A packet that yields
    /// no frames (a codec's first packet after the start or a reset can be like that) carries
    /// the pts of the next frame to come.
    pub pts: i64,
    pub duration: u64,
    pub data: Vec<u8>,
}

/// Interleaved samples. `S24` holds sign-extended 24-bit values in `i32`
/// (range -2^23..2^23-1).
#[derive(Debug, Clone, PartialEq)]
pub enum Samples {
    U8(Vec<u8>),
    S16(Vec<i16>),
    S24(Vec<i32>),
    S32(Vec<i32>),
    F32(Vec<f32>),
    F64(Vec<f64>),
}

impl Samples {
    pub fn new(format: SampleFormat) -> Self {
        match format {
            SampleFormat::U8 => Self::U8(Vec::new()),
            SampleFormat::S16 => Self::S16(Vec::new()),
            SampleFormat::S24 => Self::S24(Vec::new()),
            SampleFormat::S32 => Self::S32(Vec::new()),
            SampleFormat::F32 => Self::F32(Vec::new()),
            SampleFormat::F64 => Self::F64(Vec::new()),
        }
    }

    pub fn format(&self) -> SampleFormat {
        match self {
            Self::U8(_) => SampleFormat::U8,
            Self::S16(_) => SampleFormat::S16,
            Self::S24(_) => SampleFormat::S24,
            Self::S32(_) => SampleFormat::S32,
            Self::F32(_) => SampleFormat::F32,
            Self::F64(_) => SampleFormat::F64,
        }
    }

    /// Samples, not frames.
    pub fn len(&self) -> usize {
        match self {
            Self::U8(v) => v.len(),
            Self::S16(v) => v.len(),
            Self::S24(v) | Self::S32(v) => v.len(),
            Self::F32(v) => v.len(),
            Self::F64(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Keeps capacity.
    pub fn clear(&mut self) {
        match self {
            Self::U8(v) => v.clear(),
            Self::S16(v) => v.clear(),
            Self::S24(v) | Self::S32(v) => v.clear(),
            Self::F32(v) => v.clear(),
            Self::F64(v) => v.clear(),
        }
    }
}

/// `pts` counts sample frames since the start of the stream (time base
/// 1/`sample_rate`).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioFrame {
    pub sample_rate: u32,
    pub layout: ChannelLayout,
    pub pts: i64,
    pub samples: Samples,
}

impl AudioFrame {
    pub fn new(spec: &AudioSpec) -> Self {
        Self {
            sample_rate: spec.sample_rate(),
            layout: spec.layout().clone(),
            pts: 0,
            samples: Samples::new(spec.format()),
        }
    }

    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.layout.channels())
    }

    pub fn spec(&self) -> AudioSpec {
        // A frame is built from a valid spec; the rate is never zero.
        AudioSpec::new(self.sample_rate, self.samples.format(), self.layout.clone())
            .expect("frame holds a valid spec")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_leaves_the_optional_fields_empty() {
        let audio = AudioSpec::new(44100, SampleFormat::S16, ChannelLayout::stereo()).unwrap();
        let tb = TimeBase::new(1, 44100).unwrap();
        let s = StreamInfo::new(2, CodecId::Flac, audio.clone(), tb);
        assert_eq!((s.index, s.codec, s.time_base), (2, CodecId::Flac, tb));
        assert_eq!(s.audio, audio);
        assert_eq!(s.frames, None);
        assert!(s.extradata.is_empty());
        assert_eq!(s.bits_per_raw_sample, None);
    }
}
