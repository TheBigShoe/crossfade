//! The Ogg Opus writer (RFC 7845) fed the way stream copy feeds it: the packets, stream and
//! tags of our own reader, on files built with `crossfade_opus::testing::ogg`; then the pages
//! it wrote, and the file read back.

use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Demuxer, Error, Limits, MediaSink, Muxer, Packet,
    SampleFormat, StreamInfo, Tags, TimeBase,
};
use crossfade_format::{FormatId, create_muxer, open_demuxer};
use crossfade_opus::testing::ogg::{self as w, OggLayout};
use std::io::{Cursor, SeekFrom};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Cursor<Vec<u8>>>>);

impl Shared {
    fn contents(&self) -> Vec<u8> {
        self.0.lock().unwrap().get_ref().clone()
    }
}

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

struct Page {
    flags: u8,
    granule: i64,
    sequence: u32,
    lacing: Vec<u8>,
    body: Vec<u8>,
    crc_ok: bool,
}

const BOS: u8 = 2;
const EOS: u8 = 4;

/// An independent page parser, so the writer is not only checked by our reader.
fn pages(file: &[u8]) -> Vec<Page> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < file.len() {
        let h = &file[at..];
        assert_eq!(&h[..5], b"OggS\0");
        let n = usize::from(h[26]);
        let lacing = h[27..27 + n].to_vec();
        let len: usize = lacing.iter().map(|&l| usize::from(l)).sum();
        let total = 27 + n + len;
        let mut zeroed = h[..total].to_vec();
        zeroed[22..26].fill(0);
        out.push(Page {
            flags: h[5],
            granule: i64::from_le_bytes(h[6..14].try_into().unwrap()),
            sequence: u32::from_le_bytes(h[18..22].try_into().unwrap()),
            lacing,
            body: h[27 + n..total].to_vec(),
            crc_ok: w::ogg_crc(&zeroed) == u32::from_le_bytes(h[22..26].try_into().unwrap()),
        });
        at += total;
    }
    out
}

fn open(bytes: &[u8]) -> Box<dyn Demuxer> {
    open_demuxer(Box::new(Cursor::new(bytes.to_vec())), &Limits::default())
        .unwrap()
        .1
}

fn muxer(stream: &StreamInfo, tags: &Tags) -> (Shared, Box<dyn Muxer>) {
    let sink = Shared::default();
    let m = create_muxer(FormatId::Ogg, Box::new(sink.clone()), stream, tags).unwrap();
    (sink, m)
}

/// Stream copy: every packet of `bytes` through the writer.
fn copy(bytes: &[u8]) -> Vec<u8> {
    let mut d = open(bytes);
    let (sink, mut m) = muxer(&d.streams()[0].clone(), &d.tags().clone());
    let mut p = Packet::default();
    while d.read_packet(&mut p).unwrap() {
        m.write_packet(&p).unwrap();
    }
    m.finish().unwrap();
    sink.contents()
}

const TAGS: [&str; 3] = ["TITLE=Tone", "ARTIST=Nobody", "R128_TRACK_GAIN=-1024"];

fn file(head: &[u8], audio: &[Vec<u8>], layout: &OggLayout) -> Vec<u8> {
    w::ogg_file(head, &w::opus_tags("some encoder", &TAGS), audio, layout)
}

fn refused(stream: &StreamInfo) -> Error {
    match create_muxer(
        FormatId::Ogg,
        Box::new(Shared::default()),
        stream,
        &Tags::new(),
    ) {
        Err(e) => e,
        Ok(_) => panic!("created"),
    }
}

#[test]
fn the_identification_and_comment_headers_get_a_page_each() {
    let head = w::opus_head(2, 312, -256, None);
    let source = file(
        &head,
        &w::packets(15, true, 3, 40, 1),
        &OggLayout::default(),
    );
    let out = pages(&copy(&source));
    assert!(out.iter().all(|p| p.crc_ok));
    let sequences: Vec<u32> = out.iter().map(|p| p.sequence).collect();
    assert_eq!(sequences, (0..out.len() as u32).collect::<Vec<_>>());

    // The header as the reader handed it out, alone on the first page.
    assert_eq!((out[0].flags, out[0].granule), (BOS, 0));
    assert_eq!(out[0].body, head);
    assert_eq!(out[0].lacing, [head.len() as u8]);

    assert_eq!((out[1].flags, out[1].granule), (0, 0));
    let tags = crossfade_opus::OpusTags::parse(&out[1].body).unwrap();
    let vendor = format!("Crossfade {}", env!("CARGO_PKG_VERSION"));
    assert_eq!(tags.vendor, vendor.as_bytes());
    assert_eq!(tags.comment_count, 3);
    assert!(tags.extra.is_empty());
    assert_eq!(out[1].body, w::opus_tags(&vendor, &TAGS));

    // Audio starts on a page of its own; only the last page ends the stream.
    assert_eq!(out.len(), 3);
    assert_eq!(out[2].flags, EOS);
    assert_eq!(out[2].lacing, [41, 41, 41]);
}

#[test]
fn a_stream_the_writer_cannot_describe_is_refused() {
    let opus = |rate: u32, extradata: &[u8]| {
        let mut s = StreamInfo::new(
            0,
            CodecId::Opus,
            AudioSpec::new(rate, SampleFormat::F32, ChannelLayout::stereo()).unwrap(),
            TimeBase::new(1, i64::from(rate)).unwrap(),
        );
        s.extradata = extradata.to_vec();
        s
    };
    let head = w::opus_head(2, 312, 0, None);
    muxer(&opus(48_000, &head), &Tags::new());

    let pcm = StreamInfo::new(
        0,
        CodecId::PcmS16Le,
        AudioSpec::new(48_000, SampleFormat::S16, ChannelLayout::stereo()).unwrap(),
        TimeBase::new(1, 48_000).unwrap(),
    );
    assert_eq!(
        refused(&pcm),
        Error::Unsupported {
            what: "ogg files hold flac or opus, not pcm_s16le".into()
        }
    );
    // No header to write: an encoder has to bring one.
    assert_eq!(
        refused(&opus(48_000, &[])),
        Error::Malformed {
            what: "ogg opus: invalid header: an identification header starts with OpusHead".into()
        }
    );
    assert_eq!(
        refused(&opus(48_000, &head[..12])),
        Error::Malformed {
            what: "ogg opus: invalid header: an identification header is cut short".into()
        }
    );
    // Granule positions and pts count 48 kHz samples, whatever the encoder's input rate.
    assert_eq!(
        refused(&opus(44_100, &head)),
        Error::Unsupported {
            what: "ogg opus is timed in 48 kHz samples, not at 44100 Hz".into()
        }
    );
}
