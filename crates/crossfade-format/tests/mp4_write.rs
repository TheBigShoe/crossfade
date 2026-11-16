//! The MP4/M4A writer on ALAC: what it writes, our own reader reads back, stream, packets,
//! tags and all.

use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Error, Limits, MediaSink, Packet, SampleFormat, StreamInfo,
    Tags, TimeBase,
};
use crossfade_format::{FormatId, create_muxer, open_demuxer};
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

/// An ALAC stream of `depth` bits and `channels` channels in ALAC's layout, `frames` long.
fn alac_stream(depth: u8, channels: u8, rate: u32, frames: u64) -> StreamInfo {
    let wave = crossfade_alac::wave_layout(channels).unwrap();
    let layout = ChannelLayout::from_mask(wave.mask, u16::from(channels)).unwrap();
    let format = match depth {
        16 => SampleFormat::S16,
        32 => SampleFormat::S32,
        _ => SampleFormat::S24,
    };
    let audio = AudioSpec::new(rate, format, layout).unwrap();
    let mut s = StreamInfo::new(
        0,
        CodecId::Alac,
        audio,
        TimeBase::new(1, i64::from(rate)).unwrap(),
    );
    s.extradata = crossfade_alac::Config::new(rate, channels, depth)
        .unwrap()
        .write()
        .to_vec();
    s.frames = Some(frames);
    s.bits_per_raw_sample = (depth == 20).then_some(20);
    s
}

/// `n` packets of 4096 frames and a last of `last`, their bytes made up (a muxer does not
/// look inside).
fn packets(n: usize, last: u64) -> Vec<Packet> {
    (0..=n)
        .map(|i| Packet {
            stream: 0,
            pts: i as i64 * 4096,
            duration: if i == n { last } else { 4096 },
            data: (0..20 + (i * 37) % 300)
                .map(|b| (b * 7 + i) as u8)
                .collect(),
        })
        .filter(|p| p.duration > 0)
        .collect()
}

fn write(stream: &StreamInfo, tags: &Tags, packets: &[Packet]) -> Vec<u8> {
    let sink = Shared::default();
    let mut m = create_muxer(FormatId::Mp4, Box::new(sink.clone()), stream, tags).unwrap();
    for p in packets {
        m.write_packet(p).unwrap();
    }
    m.finish().unwrap();
    sink.contents()
}

fn read(bytes: Vec<u8>) -> (StreamInfo, Tags, Vec<Packet>) {
    let (format, mut d) = open_demuxer(Box::new(Cursor::new(bytes)), &Limits::default()).unwrap();
    assert_eq!(format, FormatId::Mp4);
    let mut all = vec![];
    let mut p = Packet::default();
    while d.read_packet(&mut p).unwrap() {
        all.push(p.clone());
    }
    (d.streams()[0].clone(), d.tags().clone(), all)
}

#[test]
fn an_alac_file_reads_back_with_its_stream_packets_and_length() {
    for (depth, channels, rate) in [
        (16u8, 2u8, 44_100u32),
        (24, 1, 96_000),
        (20, 2, 48_000),
        (32, 6, 48_000),
    ] {
        for (n, last) in [(0usize, 100u64), (1, 0), (9, 1234), (40, 4096)] {
            let sent = packets(n, last);
            let frames: u64 = sent.iter().map(|p| p.duration).sum();
            let stream = alac_stream(depth, channels, rate, frames);
            let bytes = write(&stream, &Tags::new(), &sent);
            assert_eq!(&bytes[4..12], b"ftypM4A ", "an M4A file");
            let (got, tags, read_back) = read(bytes);
            assert_eq!(got, stream, "{depth}-bit {channels} ch, {n} packets");
            assert!(tags.is_empty());
            assert!(
                read_back == sent,
                "{depth}-bit {channels} ch, {n} packets and {last}"
            );
        }
    }
}

#[test]
fn an_empty_stream_is_still_a_file_that_opens() {
    let stream = alac_stream(16, 2, 44_100, 0);
    let (got, _, read_back) = read(write(&stream, &Tags::new(), &[]));
    assert_eq!(got.frames, Some(0));
    assert!(read_back.is_empty());
}

#[test]
fn tags_are_written_as_itunes_items_and_read_back() {
    let mut tags = Tags::new();
    for (k, v) in [
        ("TITLE", "Blue in Green"),
        ("ARTIST", "Miles Davis"),
        ("ALBUMARTIST", "Miles Davis"),
        ("ALBUM", "Kind of Blue"),
        ("DATE", "1959"),
        ("GENRE", "Jazz"),
        ("TRACKNUMBER", "3"),
        ("TRACKTOTAL", "5"),
        ("DISCNUMBER", "1"),
        ("COMMENT", "Säuberlich, with accents"),
        ("COMPOSER", "Bill Evans"),
        ("REPLAYGAIN_TRACK_GAIN", "-3.2 dB"),
        ("ARTIST", "Bill Evans"),
    ] {
        tags.push(k, v);
    }
    let stream = alac_stream(16, 2, 44_100, 5000);
    let (_, got, _) = read(write(&stream, &tags, &packets(1, 904)));
    let pairs = |t: &Tags| {
        let mut v: Vec<(String, String)> =
            t.iter().map(|t| (t.key.clone(), t.value.clone())).collect();
        v.sort();
        v
    };
    assert_eq!(pairs(&got), pairs(&tags));
}

#[test]
fn the_file_seeks() {
    let sent = packets(30, 77);
    let stream = alac_stream(16, 2, 44_100, 30 * 4096 + 77);
    let bytes = write(&stream, &Tags::new(), &sent);
    let (_, mut d) = open_demuxer(Box::new(Cursor::new(bytes)), &Limits::default()).unwrap();
    assert_eq!(d.seek(4096 * 17 + 5).unwrap(), 4096 * 17);
    let mut p = Packet::default();
    assert!(d.read_packet(&mut p).unwrap());
    assert!(p == sent[17]);
}

#[test]
fn what_the_writer_cannot_hold_is_refused_before_a_byte_is_written() {
    let refused = |stream: &StreamInfo| {
        let sink = Shared::default();
        let r = create_muxer(FormatId::Mp4, Box::new(sink.clone()), stream, &Tags::new());
        assert!(sink.contents().is_empty());
        match r {
            Err(Error::Unsupported { what }) => what,
            Err(e) => panic!("{e:?}"),
            Ok(_) => panic!("accepted"),
        }
    };
    let mut flac = alac_stream(16, 2, 44_100, 0);
    flac.codec = CodecId::Flac;
    assert_eq!(refused(&flac), "mp4 files hold alac here, not flac");
    let mut bare = alac_stream(16, 2, 44_100, 0);
    bare.extradata.clear();
    assert!(
        refused(&bare).starts_with("alac: invalid config"),
        "{}",
        refused(&bare)
    );
}
