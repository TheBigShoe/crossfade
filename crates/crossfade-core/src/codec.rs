use crate::SampleFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CodecId {
    PcmU8,
    /// Stored as unsigned with a +128 offset, which is lossless.
    PcmS8,
    PcmS16Le,
    PcmS24Le,
    PcmS32Le,
    PcmF32Le,
    PcmF64Le,
    PcmS16Be,
    PcmS24Be,
    PcmS32Be,
    PcmF32Be,
    PcmF64Be,
    Flac,
}

impl CodecId {
    /// FFmpeg's name for the codec.
    pub fn name(self) -> &'static str {
        match self {
            Self::PcmU8 => "pcm_u8",
            Self::PcmS8 => "pcm_s8",
            Self::PcmS16Le => "pcm_s16le",
            Self::PcmS24Le => "pcm_s24le",
            Self::PcmS32Le => "pcm_s32le",
            Self::PcmF32Le => "pcm_f32le",
            Self::PcmF64Le => "pcm_f64le",
            Self::PcmS16Be => "pcm_s16be",
            Self::PcmS24Be => "pcm_s24be",
            Self::PcmS32Be => "pcm_s32be",
            Self::PcmF32Be => "pcm_f32be",
            Self::PcmF64Be => "pcm_f64be",
            Self::Flac => "flac",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "pcm_u8" => Self::PcmU8,
            "pcm_s8" => Self::PcmS8,
            "pcm_s16le" => Self::PcmS16Le,
            "pcm_s24le" => Self::PcmS24Le,
            "pcm_s32le" => Self::PcmS32Le,
            "pcm_f32le" => Self::PcmF32Le,
            "pcm_f64le" => Self::PcmF64Le,
            "pcm_s16be" => Self::PcmS16Be,
            "pcm_s24be" => Self::PcmS24Be,
            "pcm_s32be" => Self::PcmS32Be,
            "pcm_f32be" => Self::PcmF32Be,
            "pcm_f64be" => Self::PcmF64Be,
            "flac" => Self::Flac,
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

    /// The big-endian PCM codec carrying `format`; 8-bit is signed (AIFF).
    pub fn pcm_be_for(format: SampleFormat) -> Self {
        match format {
            SampleFormat::U8 => Self::PcmS8,
            SampleFormat::S16 => Self::PcmS16Be,
            SampleFormat::S24 => Self::PcmS24Be,
            SampleFormat::S32 => Self::PcmS32Be,
            SampleFormat::F32 => Self::PcmF32Be,
            SampleFormat::F64 => Self::PcmF64Be,
        }
    }

    pub fn is_pcm(self) -> bool {
        self.pcm_sample_format().is_some()
    }

    pub fn pcm_sample_format(self) -> Option<SampleFormat> {
        Some(match self {
            Self::PcmU8 | Self::PcmS8 => SampleFormat::U8,
            Self::PcmS16Le | Self::PcmS16Be => SampleFormat::S16,
            Self::PcmS24Le | Self::PcmS24Be => SampleFormat::S24,
            Self::PcmS32Le | Self::PcmS32Be => SampleFormat::S32,
            Self::PcmF32Le | Self::PcmF32Be => SampleFormat::F32,
            Self::PcmF64Le | Self::PcmF64Be => SampleFormat::F64,
            Self::Flac => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [CodecId; 13] = [
        CodecId::PcmU8,
        CodecId::PcmS8,
        CodecId::PcmS16Le,
        CodecId::PcmS24Le,
        CodecId::PcmS32Le,
        CodecId::PcmF32Le,
        CodecId::PcmF64Le,
        CodecId::PcmS16Be,
        CodecId::PcmS24Be,
        CodecId::PcmS32Be,
        CodecId::PcmF32Be,
        CodecId::PcmF64Be,
        CodecId::Flac,
    ];

    #[test]
    fn names_round_trip_and_follow_ffmpeg() {
        for c in ALL {
            assert_eq!(CodecId::from_name(c.name()), Some(c));
        }
        assert_eq!(CodecId::Flac.name(), "flac");
        assert_eq!(CodecId::PcmS8.name(), "pcm_s8");
        assert_eq!(CodecId::PcmS24Be.name(), "pcm_s24be");
        assert_eq!(CodecId::PcmF64Be.name(), "pcm_f64be");
        assert_eq!(CodecId::from_name("mp3"), None);
    }

    #[test]
    fn pcm_codecs_map_to_sample_formats() {
        assert_eq!(CodecId::PcmS8.pcm_sample_format(), Some(SampleFormat::U8));
        assert_eq!(CodecId::PcmS16Be.pcm_sample_format(), Some(SampleFormat::S16));
        assert_eq!(CodecId::PcmF32Be.pcm_sample_format(), Some(SampleFormat::F32));
        assert_eq!(CodecId::Flac.pcm_sample_format(), None);
        assert!(!CodecId::Flac.is_pcm());
        assert!(ALL.iter().filter(|c| c.is_pcm()).count() == 12);
    }

    #[test]
    fn endianness_helpers() {
        let formats = [
            SampleFormat::U8,
            SampleFormat::S16,
            SampleFormat::S24,
            SampleFormat::S32,
            SampleFormat::F32,
            SampleFormat::F64,
        ];
        for f in formats {
            assert_eq!(CodecId::pcm_for(f).pcm_sample_format(), Some(f));
            assert_eq!(CodecId::pcm_be_for(f).pcm_sample_format(), Some(f));
        }
        assert_eq!(CodecId::pcm_be_for(SampleFormat::U8), CodecId::PcmS8);
        assert_eq!(CodecId::pcm_be_for(SampleFormat::S16), CodecId::PcmS16Be);
        assert_eq!(CodecId::pcm_for(SampleFormat::U8), CodecId::PcmU8);
    }
}
