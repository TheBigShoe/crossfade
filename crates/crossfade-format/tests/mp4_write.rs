//! The MP4 writer against the reader, on the ALAC files `afconvert` made (see
//! `tests/fixtures/mp4/README.md`): what the reader gives, the writer takes, and the reader
//! gives the same again.

use crossfade_core::{Error, Limits, MediaSink, Muxer, Packet, StreamInfo, Tags};
use crossfade_format::{FormatId, create_muxer, open_demuxer};
use std::io::{Cursor, SeekFrom};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Cursor<Vec<u8>>>>);

impl Shared {
    fn bytes(&self) -> Vec<u8> {
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

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/mp4/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// A file as the reader gives it.
struct Read {
    format: FormatId,
    stream: StreamInfo,
    packets: Vec<Packet>,
    tags: Tags,
}

fn read(bytes: Vec<u8>) -> Read {
    let (format, mut d) = open_demuxer(Box::new(Cursor::new(bytes)), &Limits::default()).unwrap();
    let mut packets = Vec::new();
    let mut p = Packet::default();
    while d.read_packet(&mut p).unwrap() {
        packets.push(p.clone());
    }
    Read {
        format,
        stream: d.streams()[0].clone(),
        packets,
        tags: d.tags().clone(),
    }
}

fn open_muxer(stream: &StreamInfo, tags: &Tags) -> Result<(Box<dyn Muxer>, Shared), Error> {
    let sink = Shared::default();
    let muxer = create_muxer(FormatId::Mp4, Box::new(sink.clone()), stream, tags)?;
    Ok((muxer, sink))
}

fn write(stream: &StreamInfo, packets: &[Packet], tags: &Tags) -> Vec<u8> {
    let (mut muxer, sink) = open_muxer(stream, tags).unwrap();
    for p in packets {
        muxer.write_packet(p).unwrap();
    }
    muxer.finish().unwrap();
    sink.bytes()
}

fn rewrite(original: &Read) -> Read {
    read(write(&original.stream, &original.packets, &original.tags))
}

fn assert_same(a: &Read, b: &Read, what: &str) {
    assert_eq!(a.stream, b.stream, "{what}: stream");
    assert_eq!(a.packets, b.packets, "{what}: packets");
    assert_eq!(a.tags, b.tags, "{what}: tags");
}

#[test]
fn a_rewritten_alac_file_reads_back_the_same() {
    for name in ["alac-stereo16.m4a", "alac-mono16.m4a"] {
        let original = read(fixture(name));
        assert!(!original.packets.is_empty(), "{name}");
        let again = rewrite(&original);
        assert_eq!(again.format, FormatId::Mp4);
        assert_same(&original, &again, name);
    }
}

#[test]
fn tags_survive_in_the_written_file() {
    let original = read(fixture("alac-stereo16.m4a"));
    let mut tags = Tags::new();
    tags.push("TITLE", "Rewritten");
    tags.push("ARTIST", "A");
    tags.push("ARTIST", "B");
    tags.push("TRACKNUMBER", "2");
    tags.push("TRACKTOTAL", "9");
    tags.push("REPLAYGAIN_TRACK_GAIN", "-3.1 dB");
    let again = read(write(&original.stream, &original.packets, &tags));
    assert_eq!(again.tags, tags);
}
