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

const ALAC_FIXTURES: [&str; 12] = [
    "alac-mono16.m4a",
    "alac-stereo16.m4a",
    "alac-stereo20.m4a",
    "alac-stereo24.m4a",
    "alac-stereo32.m4a",
    "alac-ch3.m4a",
    "alac-ch4.m4a",
    "alac-ch5.m4a",
    "alac-ch6.m4a",
    "alac-ch7.m4a",
    "alac-ch8.m4a",
    "alac-noise16.m4a",
];

/// The boxes in a box body: (type, body).
fn children(body: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut rest = body;
    while rest.len() >= 8 {
        let size = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
        assert!((8..=rest.len()).contains(&size), "box size {size}");
        out.push((rest[4..8].try_into().unwrap(), &rest[8..size]));
        rest = &rest[size..];
    }
    assert!(rest.is_empty(), "{} stray bytes", rest.len());
    out
}

fn child<'a>(body: &'a [u8], kind: &[u8; 4]) -> &'a [u8] {
    children(body)
        .into_iter()
        .find(|(k, _)| k == kind)
        .unwrap_or_else(|| panic!("no {}", String::from_utf8_lossy(kind)))
        .1
}

/// The boxes inside the track's `alac` sample entry: its cookie box and the `chan` box.
fn entry_children(file: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
    let mut body = child(file, b"moov");
    for kind in [b"trak", b"mdia", b"minf", b"stbl"] {
        body = child(body, kind);
    }
    let stsd = child(body, b"stsd");
    let entry = child(&stsd[8..], b"alac");
    children(&entry[28..])
        .into_iter()
        .map(|(k, b)| (k, b.to_vec()))
        .collect()
}

#[test]
fn every_alac_fixture_reads_back_the_same_after_a_rewrite() {
    for name in ALAC_FIXTURES {
        let original = read(fixture(name));
        assert_same(&original, &rewrite(&original), name);
    }
}

fn total_frames(packets: &[Packet]) -> u64 {
    packets.iter().map(|p| p.duration).sum()
}

fn message(e: Error) -> String {
    match e {
        Error::Unsupported { what } | Error::Malformed { what } => what,
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn a_stream_shorter_than_its_packets_is_cut_by_an_edit_list() {
    let original = read(fixture("alac-stereo16.m4a"));
    let total = total_frames(&original.packets);
    let mut stream = original.stream.clone();
    stream.frames = Some(total - 100);
    let again = read(write(&stream, &original.packets, &original.tags));
    assert_eq!(again.stream.frames, Some(total - 100));
    assert_eq!(again.packets, original.packets);
}

#[test]
fn a_stream_with_no_length_plays_whole() {
    let original = read(fixture("alac-stereo16.m4a"));
    let mut stream = original.stream.clone();
    stream.frames = None;
    let again = read(write(&stream, &original.packets, &original.tags));
    assert_eq!(again.stream.frames, Some(total_frames(&original.packets)));
}

#[test]
fn priming_frames_before_pts_zero_come_back_as_negative_pts() {
    let original = read(fixture("alac-stereo16.m4a"));
    let mut packets = original.packets.clone();
    for p in &mut packets {
        p.pts -= 1000;
    }
    let mut stream = original.stream.clone();
    stream.frames = Some(total_frames(&packets) - 1000);
    let again = read(write(&stream, &packets, &original.tags));
    assert_eq!(again.packets, packets);
    assert_eq!(again.stream.frames, stream.frames);
}

#[test]
fn a_length_the_packets_cannot_back_is_refused_not_written_short() {
    let original = read(fixture("alac-stereo16.m4a"));
    let mut stream = original.stream.clone();
    stream.frames = Some(total_frames(&original.packets) + 1);
    let (mut muxer, _) = open_muxer(&stream, &original.tags).unwrap();
    for p in &original.packets {
        muxer.write_packet(p).unwrap();
    }
    let e = message(muxer.finish().unwrap_err());
    assert!(e.contains("10008 frames") && e.contains("10007"), "{e}");
}

#[test]
fn packets_must_follow_one_another_on_the_timeline() {
    let original = read(fixture("alac-stereo16.m4a"));
    let (mut muxer, _) = open_muxer(&original.stream, &original.tags).unwrap();
    muxer.write_packet(&original.packets[0]).unwrap();
    let e = message(muxer.write_packet(&original.packets[2]).unwrap_err());
    assert!(e.contains("follow"), "{e}");
    let (mut muxer, _) = open_muxer(&original.stream, &original.tags).unwrap();
    let mut late = original.packets[0].clone();
    late.pts = 5;
    let e = message(muxer.write_packet(&late).unwrap_err());
    assert!(e.contains("starts after 0"), "{e}");
}

/// A sink that cannot seek, like a pipe.
struct Pipe;

impl MediaSink for Pipe {
    fn write_all(&mut self, _: &[u8]) -> std::io::Result<()> {
        panic!("nothing should be written before the refusal");
    }
    fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
        Err(std::io::Error::from(std::io::ErrorKind::NotSeekable))
    }
    fn finish(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_sink_that_cannot_seek_is_refused_up_front() {
    let original = read(fixture("alac-stereo16.m4a"));
    let e = create_muxer(
        FormatId::Mp4,
        Box::new(Pipe),
        &original.stream,
        &original.tags,
    );
    let e = message(e.err().unwrap());
    assert!(e.contains("mp4") && e.contains("can seek"), "{e}");
}

#[test]
fn other_codecs_are_refused_by_name_before_anything_is_written() {
    use crossfade_core::{AudioSpec, ChannelLayout, CodecId, SampleFormat, TimeBase};
    let audio = AudioSpec::new(44100, SampleFormat::F32, ChannelLayout::stereo()).unwrap();
    for codec in [CodecId::Aac, CodecId::Mp3, CodecId::Flac] {
        let stream = StreamInfo::new(0, codec, audio.clone(), TimeBase::new(1, 44100).unwrap());
        let e = open_muxer(&stream, &Tags::new()).err().unwrap();
        assert_eq!(
            message(e),
            format!("writing {} into mp4 is not supported yet", codec.name())
        );
        let e = create_muxer(FormatId::Mp4, Box::new(Pipe), &stream, &Tags::new());
        assert!(e.is_err());
    }
}

#[test]
fn a_stream_with_no_packets_is_a_file_with_no_frames() {
    let original = read(fixture("alac-stereo16.m4a"));
    let mut stream = original.stream.clone();
    stream.frames = Some(0);
    let again = read(write(&stream, &[], &Tags::new()));
    assert_eq!(again.stream.frames, Some(0));
    assert!(again.packets.is_empty());
    assert_eq!(again.stream.extradata, original.stream.extradata);
}

fn config_of(stream: &StreamInfo) -> crossfade_alac::Config {
    crossfade_alac::Config::parse(&stream.extradata).unwrap()
}

#[test]
fn set_extradata_replaces_the_stored_cookie() {
    let original = read(fixture("alac-stereo16.m4a"));
    let mut cookie = config_of(&original.stream);
    cookie.max_frame_bytes = 12345;
    cookie.avg_bit_rate = 678;
    let mut stream = original.stream.clone();
    // The encoder's provisional header, then its final one.
    stream.extradata = crossfade_alac::Config {
        max_frame_bytes: 0,
        avg_bit_rate: 0,
        ..cookie
    }
    .write()
    .to_vec();
    let (mut muxer, sink) = open_muxer(&stream, &Tags::new()).unwrap();
    for p in &original.packets {
        muxer.write_packet(p).unwrap();
    }
    muxer.set_extradata(&cookie.write()).unwrap();
    muxer.finish().unwrap();
    let again = read(sink.bytes());
    assert_eq!(again.stream.extradata, cookie.write());
    assert_eq!(config_of(&again.stream).max_frame_bytes, 12345);
}

#[test]
fn a_cookie_may_come_only_with_set_extradata_but_must_come() {
    let original = read(fixture("alac-stereo16.m4a"));
    let mut stream = original.stream.clone();
    stream.extradata.clear();
    let (mut muxer, _) = open_muxer(&stream, &Tags::new()).unwrap();
    muxer.write_packet(&original.packets[0]).unwrap();
    let e = message(muxer.finish().unwrap_err());
    assert!(e.contains("cookie"), "{e}");
    let (mut muxer, sink) = open_muxer(&stream, &Tags::new()).unwrap();
    muxer.write_packet(&original.packets[0]).unwrap();
    muxer.set_extradata(&original.stream.extradata).unwrap();
    muxer.finish().unwrap();
    assert_eq!(
        read(sink.bytes()).stream.extradata,
        original.stream.extradata
    );
}

#[test]
fn a_cookie_that_disagrees_with_the_stream_is_refused() {
    let original = read(fixture("alac-stereo16.m4a"));
    let good = config_of(&original.stream);
    let cases = [
        (
            "channels",
            crossfade_alac::Config {
                channels: 1,
                ..good
            },
        ),
        (
            "rate",
            crossfade_alac::Config {
                sample_rate: 48000,
                ..good
            },
        ),
        (
            "depth",
            crossfade_alac::Config {
                bit_depth: 24,
                ..good
            },
        ),
    ];
    for (what, config) in cases {
        let mut stream = original.stream.clone();
        stream.extradata = config.write().to_vec();
        let e = message(open_muxer(&stream, &Tags::new()).err().unwrap());
        assert!(e.contains(what), "{what}: {e}");
        // The same through `set_extradata`.
        let (mut muxer, _) = open_muxer(&original.stream, &Tags::new()).unwrap();
        let e = message(muxer.set_extradata(&config.write()).unwrap_err());
        assert!(e.contains(what), "{what}: {e}");
    }
    let mut stream = original.stream.clone();
    stream.extradata.truncate(20);
    assert!(open_muxer(&stream, &Tags::new()).is_err());
}

#[test]
fn a_layout_other_than_alacs_own_is_refused() {
    use crossfade_core::{AudioSpec, ChannelLayout};
    let original = read(fixture("alac-ch6.m4a"));
    let mut stream = original.stream.clone();
    let layout = ChannelLayout::unpositioned(6).unwrap();
    stream.audio =
        AudioSpec::new(stream.audio.sample_rate(), stream.audio.format(), layout).unwrap();
    let e = message(open_muxer(&stream, &Tags::new()).err().unwrap());
    assert!(e.contains("layout") && e.contains("6 channels"), "{e}");
}

#[test]
fn a_cookie_with_its_chan_atom_is_stored_whole() {
    let original = read(fixture("alac-ch6.m4a"));
    let mut stream = original.stream.clone();
    let mut cookie = stream.extradata.clone();
    // A tag of its own, to tell it from the one the writer would make.
    let chan = [
        &24u32.to_be_bytes()[..],
        b"chan",
        &[0; 4],
        &0x780006u32.to_be_bytes(),
        &[0; 8],
    ]
    .concat();
    cookie.extend_from_slice(&chan);
    stream.extradata = cookie;
    let file = write(&stream, &original.packets, &Tags::new());
    let stored = entry_children(&file);
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[1], (*b"chan", chan[8..].to_vec()));
    assert_eq!(read(file).stream.extradata, original.stream.extradata);
}

#[test]
fn layouts_above_stereo_are_stored_as_afconvert_stores_them() {
    for name in ALAC_FIXTURES {
        let bytes = fixture(name);
        let original = read(bytes.clone());
        let written = write(&original.stream, &original.packets, &original.tags);
        assert_eq!(entry_children(&written), entry_children(&bytes), "{name}");
    }
}
