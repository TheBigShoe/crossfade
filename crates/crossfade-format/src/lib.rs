//! Container formats: demuxers that split a byte stream into packets and
//! muxers that write them back. Built on the seams in `crossfade-core`.
#![forbid(unsafe_code)]

pub mod adts;
pub mod aiff;
pub mod flac;
pub mod mp3;
pub mod mp4;
pub mod ogg;
mod replay;
mod sniff;
pub mod vorbis_comment;
pub mod wav;

use crossfade_core::{Demuxer, Error, Limits, MediaSink, MediaSource, Muxer, StreamInfo, Tags};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FormatId {
    Wav,
    Flac,
    Aiff,
    Ogg,
    /// MPEG audio frames (layers I-III), optionally behind ID3v2 tags.
    Mp3,
    /// ISO base media (MP4, M4A, M4B).
    Mp4,
    /// AAC in ADTS frames.
    Adts,
}

impl FormatId {
    pub fn name(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Aiff => "aiff",
            Self::Ogg => "ogg",
            Self::Mp3 => "mp3",
            Self::Mp4 => "mp4",
            Self::Adts => "adts",
        }
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "wav" | "wave" => Some(Self::Wav),
            "flac" => Some(Self::Flac),
            "aiff" | "aif" | "aifc" => Some(Self::Aiff),
            "ogg" | "oga" | "opus" => Some(Self::Ogg),
            "mp3" => Some(Self::Mp3),
            "mp4" | "m4a" | "m4b" => Some(Self::Mp4),
            "aac" | "adts" => Some(Self::Adts),
            _ => None,
        }
    }
}

/// Identifies the container from its first bytes without seeking, so a pipe works: the
/// chosen demuxer gets those bytes back in front of the rest.
pub fn open_demuxer(
    source: Box<dyn MediaSource>,
    limits: &Limits,
) -> Result<(FormatId, Box<dyn Demuxer>), Error> {
    let sniffed = sniff::sniff(source, limits)?;
    let format = sniffed.format;
    let source: Box<dyn MediaSource> = Box::new(sniffed.source);
    let demuxer: Box<dyn Demuxer> = match format {
        FormatId::Wav => Box::new(wav::WavDemuxer::new(source, limits)?),
        FormatId::Flac => flac::open(source, limits)?,
        FormatId::Aiff => aiff::open(source, limits)?,
        FormatId::Ogg => ogg::open(source, limits)?,
        FormatId::Mp3 => mp3::open(source, sniffed.id3, limits)?,
        FormatId::Mp4 => mp4::open(source, limits)?,
        FormatId::Adts => adts::open(source, limits)?,
    };
    Ok((format, demuxer))
}

/// `tags` are written where the container can hold them.
pub fn create_muxer(
    format: FormatId,
    sink: Box<dyn MediaSink>,
    stream: &StreamInfo,
    tags: &Tags,
) -> Result<Box<dyn Muxer>, Error> {
    match format {
        FormatId::Wav => Ok(Box::new(wav::WavMuxer::with_tags(sink, stream, tags)?)),
        FormatId::Flac => flac::create(sink, stream, tags),
        FormatId::Aiff => aiff::create(sink, stream, tags),
        FormatId::Ogg => ogg::create(sink, stream, tags),
        FormatId::Mp3 | FormatId::Mp4 | FormatId::Adts => Err(Error::Unsupported {
            what: format!("writing {} is not supported yet", format.name()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossfade_core::{
        AudioSpec, ChannelLayout, CodecId, Error, Limits, MediaSink, Packet, SampleFormat,
        StreamInfo, Tags, TimeBase,
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
        StreamInfo::new(0, CodecId::PcmS16Le, audio, TimeBase::new(1, 8000).unwrap())
    }

    fn wav_bytes() -> Vec<u8> {
        let sink = Shared::default();
        let mut m = create_muxer(
            FormatId::Wav,
            Box::new(sink.clone()),
            &stream(),
            &Tags::new(),
        )
        .unwrap();
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
        for (id, name, exts) in [
            (FormatId::Wav, "wav", &["wav", "WAV", "Wave", "wave"][..]),
            (FormatId::Flac, "flac", &["flac", "FLAC"][..]),
            (FormatId::Aiff, "aiff", &["aiff", "aif", "aifc", "AIF"][..]),
            (
                FormatId::Ogg,
                "ogg",
                &["ogg", "oga", "OGG", "opus", "Opus"][..],
            ),
            (FormatId::Mp3, "mp3", &["mp3", "MP3"][..]),
            (FormatId::Mp4, "mp4", &["mp4", "m4a", "m4b", "M4A"][..]),
            (FormatId::Adts, "adts", &["aac", "adts", "AAC"][..]),
        ] {
            assert_eq!(id.name(), name);
            for ext in exts {
                assert_eq!(FormatId::from_extension(ext), Some(id), "{ext}");
            }
        }
        for ext in [
            "", "mp2", "wav ", ".wav", "wavx", "flc", "ogv", "m4v", "mov",
        ] {
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

    fn unsupported(r: Result<impl Sized, Error>) -> String {
        match r {
            Err(Error::Unsupported { what }) => what,
            Err(e) => panic!("wrong error {e:?}"),
            Ok(_) => panic!("expected Unsupported"),
        }
    }

    #[test]
    fn magic_routes_to_the_right_demuxer() {
        let open = |bytes: &[u8]| {
            let src = FaultySource::new(Cursor::new(bytes.to_vec())).not_seekable();
            open_demuxer(Box::new(src), &Limits::default()).map(|(id, _)| id)
        };
        // Every container has a reader now; each one's routing is also tested in
        // its own module. The FLAC reader got these: it ran out of input inside
        // STREAMINFO.
        for bytes in [
            &b"fLaC\0\0\0\x22rest"[..],
            b"ID3\x04\0\0\0\0\0\0fLaC\0\0\0\x22",
        ] {
            assert!(
                matches!(open(bytes), Err(Error::UnexpectedEof { .. })),
                "{bytes:?}"
            );
        }
        let wav = wav_bytes();
        assert_eq!(open(&wav).unwrap(), FormatId::Wav);
    }

    #[test]
    fn the_new_containers_reach_their_readers_which_say_they_are_not_built_yet() {
        let open = |bytes: Vec<u8>| {
            let src = FaultySource::new(Cursor::new(bytes)).not_seekable();
            unsupported(open_demuxer(Box::new(src), &Limits::default()))
        };
        let mp3 = vec![0xFF, 0xFB, 0x90, 0x00, 0, 0];
        let mut tagged = b"ID3\x03\0\0\0\0\0\x02xx".to_vec();
        tagged.extend(&mp3);
        for (bytes, name) in [
            (mp3, "mp3"),
            (tagged, "mp3"),
            (b"\0\0\0\x18ftypmp42\0\0\0\0".to_vec(), "mp4"),
            (vec![0xFF, 0xF1, 0x50, 0x80, 0, 0], "adts"),
        ] {
            assert_eq!(open(bytes), format!("reading {name} is not built yet"));
        }
    }

    #[test]
    fn unknown_input_is_unrecognised_and_rf64_is_unsupported() {
        let open = |bytes: Vec<u8>| open_demuxer(Box::new(Cursor::new(bytes)), &Limits::default());
        for bytes in [
            Vec::new(),
            b"fLa".to_vec(),
            b"FORM\0\0\0\x10".to_vec(),
            b"FORM\0\0\0\x108SVXVHDR".to_vec(),
            b"ID".to_vec(),
            b"hello world, not audio".to_vec(),
        ] {
            assert_eq!(
                open(bytes.clone()).err(),
                Some(Error::UnrecognisedFormat),
                "{bytes:?}"
            );
        }
        let mut rf64 = wav_bytes();
        rf64[..4].copy_from_slice(b"RF64");
        assert!(matches!(open(rf64).err(), Some(Error::Unsupported { .. })));
    }

    #[test]
    fn each_muxer_refuses_a_stream_it_cannot_hold() {
        let mk = |f| create_muxer(f, Box::new(Shared::default()), &stream(), &Tags::new());
        assert!(mk(FormatId::Wav).is_ok());
        // The FLAC writer exists; this PCM stream is simply not one it can hold.
        assert!(unsupported(mk(FormatId::Flac)).contains("flac files cannot hold"));
        assert!(unsupported(mk(FormatId::Ogg)).contains("ogg files can only hold flac"));
        for f in [FormatId::Mp3, FormatId::Mp4, FormatId::Adts] {
            assert_eq!(
                unsupported(mk(f)),
                format!("writing {} is not supported yet", f.name())
            );
        }
    }
}
