//! Container formats: demuxers that split a byte stream into packets and
//! muxers that write them back. Built on the seams in `crossfade-core`.
#![forbid(unsafe_code)]

pub mod wav;

use crossfade_core::{Demuxer, Error, Limits, MediaSink, MediaSource, Muxer, StreamInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FormatId {
    Wav,
}

impl FormatId {
    pub fn name(self) -> &'static str {
        match self {
            Self::Wav => "wav",
        }
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "wav" | "wave" => Some(Self::Wav),
            _ => None,
        }
    }
}

/// Identifies the container by its magic. Each demuxer checks its own, so
/// nothing needs to seek back on a pipe.
pub fn open_demuxer(
    source: Box<dyn MediaSource>,
    limits: &Limits,
) -> Result<(FormatId, Box<dyn Demuxer>), Error> {
    let demuxer = wav::WavDemuxer::new(source, limits)?;
    Ok((FormatId::Wav, Box::new(demuxer)))
}

pub fn create_muxer(
    format: FormatId,
    sink: Box<dyn MediaSink>,
    stream: &StreamInfo,
) -> Result<Box<dyn Muxer>, Error> {
    match format {
        FormatId::Wav => Ok(Box::new(wav::WavMuxer::new(sink, stream)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossfade_core::{
        AudioSpec, ChannelLayout, CodecId, Error, Limits, MediaSink, Packet, SampleFormat,
        StreamInfo, TimeBase,
    };
    use crossfade_testkit::io::FaultySource;
    use std::io::{Cursor, SeekFrom};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Cursor<Vec<u8>>>>);

    impl MediaSink for Shared {
        fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
            std::io::Write::write_all(&mut *self.0.lock().unwrap(), buf)
        }
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            std::io::Seek::seek(&mut *self.0.lock().unwrap(), pos)
        }
        fn finish(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn stream() -> StreamInfo {
        let audio = AudioSpec::new(8000, SampleFormat::S16, ChannelLayout::mono()).unwrap();
        StreamInfo {
            index: 0,
            codec: CodecId::PcmS16Le,
            audio,
            time_base: TimeBase::new(1, 8000).unwrap(),
            frames: None,
        }
    }

    fn wav_bytes() -> Vec<u8> {
        let sink = Shared::default();
        let mut m = create_muxer(FormatId::Wav, Box::new(sink.clone()), &stream()).unwrap();
        let p = Packet {
            data: vec![1, 0, 2, 0],
            ..Packet::default()
        };
        m.write_packet(&p).unwrap();
        m.finish().unwrap();
        let guard = sink.0.lock().unwrap();
        guard.get_ref().clone()
    }

    #[test]
    fn names_and_extensions() {
        assert_eq!(FormatId::Wav.name(), "wav");
        for ext in ["wav", "WAV", "Wave", "wave"] {
            assert_eq!(FormatId::from_extension(ext), Some(FormatId::Wav), "{ext}");
        }
        for ext in ["", "mp3", "wav ", ".wav", "wavx"] {
            assert_eq!(FormatId::from_extension(ext), None, "{ext:?}");
        }
    }

    #[test]
    fn sniffing_finds_wav_even_on_a_pipe() {
        let src = FaultySource::new(Cursor::new(wav_bytes())).not_seekable();
        let (id, mut d) = open_demuxer(Box::new(src), &Limits::default()).unwrap();
        assert_eq!(id, FormatId::Wav);
        assert_eq!(d.streams()[0].frames, Some(2));
        let mut p = Packet::default();
        assert!(d.read_packet(&mut p).unwrap());
        assert_eq!(p.data, [1, 0, 2, 0]);
    }

    #[test]
    fn unknown_input_is_unrecognised_and_rf64_is_unsupported() {
        let open = |bytes: Vec<u8>| open_demuxer(Box::new(Cursor::new(bytes)), &Limits::default());
        assert_eq!(
            open(b"OggS....more".to_vec()).err(),
            Some(Error::UnrecognisedFormat)
        );
        assert_eq!(open(Vec::new()).err(), Some(Error::UnrecognisedFormat));
        let mut rf64 = wav_bytes();
        rf64[..4].copy_from_slice(b"RF64");
        assert!(matches!(open(rf64).err(), Some(Error::Unsupported { .. })));
    }
}
