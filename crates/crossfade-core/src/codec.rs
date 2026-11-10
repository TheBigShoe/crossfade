use crate::SampleFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CodecId {
    PcmU8,
    PcmS16Le,
    PcmS24Le,
    PcmS32Le,
    PcmF32Le,
    PcmF64Le,
}

impl CodecId {
    /// FFmpeg's name for the codec.
    pub fn name(self) -> &'static str {
        match self {
            Self::PcmU8 => "pcm_u8",
            Self::PcmS16Le => "pcm_s16le",
            Self::PcmS24Le => "pcm_s24le",
            Self::PcmS32Le => "pcm_s32le",
            Self::PcmF32Le => "pcm_f32le",
            Self::PcmF64Le => "pcm_f64le",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "pcm_u8" => Self::PcmU8,
            "pcm_s16le" => Self::PcmS16Le,
            "pcm_s24le" => Self::PcmS24Le,
            "pcm_s32le" => Self::PcmS32Le,
            "pcm_f32le" => Self::PcmF32Le,
            "pcm_f64le" => Self::PcmF64Le,
            _ => return None,
        })
    }

    /// The little-endian PCM codec carrying `format`.
    pub fn pcm_for(format: SampleFormat) -> Self {
        match format {
            SampleFormat::U8 => Self::PcmU8,
            SampleFormat::S16 => Self::PcmS16Le,
            SampleFormat::S24 => Self::PcmS24Le,
            SampleFormat::S32 => Self::PcmS32Le,
            SampleFormat::F32 => Self::PcmF32Le,
            SampleFormat::F64 => Self::PcmF64Le,
        }
    }

    pub fn pcm_sample_format(self) -> Option<SampleFormat> {
        Some(match self {
            Self::PcmU8 => SampleFormat::U8,
            Self::PcmS16Le => SampleFormat::S16,
            Self::PcmS24Le => SampleFormat::S24,
            Self::PcmS32Le => SampleFormat::S32,
            Self::PcmF32Le => SampleFormat::F32,
            Self::PcmF64Le => SampleFormat::F64,
        })
    }
}
