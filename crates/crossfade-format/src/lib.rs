//! Container formats: demuxers that split a byte stream into packets and
//! muxers that write them back. Built on the seams in `crossfade-core`.
#![forbid(unsafe_code)]

pub mod aiff;
pub mod flac;
pub mod ogg;
mod replay;
pub mod vorbis_comment;
pub mod wav;

use crossfade_core::{Demuxer, Error, Limits, MediaSink, MediaSource, Muxer, StreamInfo, Tags};
use replay::Replay;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FormatId {
    Wav,
    Flac,
    Aiff,
    Ogg,
}

impl FormatId {
    pub fn name(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Aiff => "aiff",
            Self::Ogg => "ogg",
        }
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "wav" | "wave" => Some(Self::Wav),
            "flac" => Some(Self::Flac),
            "aiff" | "aif" | "aifc" => Some(Self::Aiff),
            "ogg" | "oga" => Some(Self::Ogg),
            _ => None,
        }
    }
}

/// The longest magic: `FORM` + size + `AIFF`.
const SNIFF_LEN: usize = 12;

fn sniff(head: &[u8]) -> Option<FormatId> {
    match head {
        [b'R', b'I', b'F', b'F', ..] | [b'R', b'F', b'6', b'4', ..] => Some(FormatId::Wav),
        // An ID3v2 tag may precede the stream marker; the FLAC reader skips it.
        [b'f', b'L', b'a', b'C', ..] | [b'I', b'D', b'3', ..] => Some(FormatId::Flac),
        [
            b'F',
            b'O',
            b'R',
            b'M',
            _,
            _,
            _,
            _,
            b'A',
            b'I',
            b'F',
            b'F' | b'C',
            ..,
        ] => Some(FormatId::Aiff),
        [b'O', b'g', b'g', b'S', ..] => Some(FormatId::Ogg),
        _ => None,
    }
}

/// Identifies the container from its first bytes without seeking, so a pipe works: the
/// chosen demuxer gets those bytes back in front of the rest.
pub fn open_demuxer(
    source: Box<dyn MediaSource>,
    limits: &Limits,
) -> Result<(FormatId, Box<dyn Demuxer>), Error> {
    let (head, source) = Replay::sniff(source, SNIFF_LEN)?;
    let format = sniff(&head).ok_or(Error::UnrecognisedFormat)?;
    let source: Box<dyn MediaSource> = Box::new(source);
    let demuxer: Box<dyn Demuxer> = match format {
        FormatId::Wav => Box::new(wav::WavDemuxer::new(source, limits)?),
        FormatId::Flac => flac::open(source, limits)?,
        FormatId::Aiff => aiff::open(source, limits)?,
        FormatId::Ogg => ogg::open(source, limits)?,
    };
    Ok((format, demuxer))
}

/// `tags` are written where the container can hold them; WAV does not yet.
pub fn create_muxer(
    format: FormatId,
    sink: Box<dyn MediaSink>,
    stream: &StreamInfo,
    tags: &Tags,
) -> Result<Box<dyn Muxer>, Error> {
    match format {
        FormatId::Wav => Ok(Box::new(wav::WavMuxer::new(sink, stream)?)),
        FormatId::Flac => flac::create(sink, stream, tags),
        FormatId::Aiff => aiff::create(sink, stream, tags),
        FormatId::Ogg => Err(Error::Unsupported {
            what: "writing ogg is not built yet".into(),
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
            (FormatId::Ogg, "ogg", &["ogg", "oga", "OGG"][..]),
        ] {
            assert_eq!(id.name(), name);
            for ext in exts {
                assert_eq!(FormatId::from_extension(ext), Some(id), "{ext}");
            }
        }
        for ext in ["", "mp3", "wav ", ".wav", "wavx", "flc", "ogv"] {
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
        for (bytes, want) in [
            (&b"fLaC\0\0\0\x22rest"[..], "reading flac is not built yet"),
            (b"ID3\x04\0\0\0\0\0\0", "reading flac is not built yet"),
            (b"FORM\0\0\0\x10AIFFCOMM", "reading aiff is not built yet"),
            (b"FORM\0\0\0\x10AIFCFVER", "reading aiff is not built yet"),
            (b"OggS\0\x02rest of page", "reading ogg is not built yet"),
        ] {
            assert_eq!(unsupported(open(bytes)), want, "{bytes:?}");
        }
        let wav = wav_bytes();
        assert_eq!(open(&wav).unwrap(), FormatId::Wav);
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
    fn muxers_exist_for_wav_only_so_far() {
        let mk = |f| create_muxer(f, Box::new(Shared::default()), &stream(), &Tags::new());
        assert!(mk(FormatId::Wav).is_ok());
        assert_eq!(
            unsupported(mk(FormatId::Flac)),
            "writing flac is not built yet"
        );
        assert_eq!(
            unsupported(mk(FormatId::Aiff)),
            "writing aiff is not built yet"
        );
        assert_eq!(
            unsupported(mk(FormatId::Ogg)),
            "writing ogg is not built yet"
        );
    }
}
