//! The Ogg Opus writer (RFC 7845) fed the way stream copy feeds it: the packets, stream and
//! tags of our own reader, on files built with `crossfade_opus::testing::ogg`; then the pages
//! it wrote, and the file read back.

use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Demuxer, Error, Limits, MediaSink, Muxer, Packet,
    SampleFormat, StreamInfo, Tags, TimeBase,
};
use crossfade_format::{FormatId, create_muxer, open_demuxer};
use crossfade_opus::testing::ogg::{self as w, OggLayout};
use crossfade_testkit::io::FaultySource;
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

const CONTINUED: u8 = 1;
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

/// What a reader shows of a file: the stream, the tags, every packet.
#[derive(PartialEq)]
struct Read {
    stream: StreamInfo,
    tags: Vec<(String, String)>,
    packets: Vec<Packet>,
}

fn read_from(mut d: Box<dyn Demuxer>) -> Read {
    let mut packets = vec![];
    let mut p = Packet::default();
    while d.read_packet(&mut p).unwrap() {
        packets.push(p.clone());
    }
    Read {
        stream: d.streams()[0].clone(),
        tags: d
            .tags()
            .iter()
            .map(|t| (t.key.to_string(), t.value.to_string()))
            .collect(),
        packets,
    }
}

/// `assert_eq!` on two reads, part by part so that a failure names what differs.
#[track_caller]
fn assert_same(got: &Read, want: &Read) {
    assert_eq!(got.stream, want.stream);
    assert_eq!(got.tags, want.tags);
    let times =
        |r: &Read| -> Vec<(i64, u64)> { r.packets.iter().map(|p| (p.pts, p.duration)).collect() };
    assert_eq!(times(got), times(want));
    assert!(got.packets == want.packets, "packet data differs");
}

fn read(bytes: &[u8]) -> Read {
    read_from(open(bytes))
}

fn muxer(stream: &StreamInfo, tags: &Tags) -> (Shared, Box<dyn Muxer>) {
    let sink = Shared::default();
    let m = create_muxer(FormatId::Ogg, Box::new(sink.clone()), stream, tags).unwrap();
    (sink, m)
}

/// Stream copy: every packet of `bytes` through the writer.
fn copy(bytes: &[u8]) -> Vec<u8> {
    copy_from(open(bytes))
}

fn copy_from(mut d: Box<dyn Demuxer>) -> Vec<u8> {
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

/// A mix of SILK, hybrid and CELT packets of 2.5 to 60 ms.
fn mixed() -> Vec<Vec<u8>> {
    let mut a = vec![];
    for (i, &(config, n)) in [(9, 3), (3, 2), (15, 4), (16, 3), (29, 2), (31, 5), (1, 2)]
        .iter()
        .enumerate()
    {
        a.extend(w::packets(config, true, n, 40, i as u64));
    }
    a
}

fn total(audio: &[Vec<u8>]) -> i64 {
    audio
        .iter()
        .map(|p| crossfade_opus::packet_duration(p).unwrap() as i64)
        .sum()
}

#[test]
fn a_copy_reads_back_with_the_same_packets_timeline_length_and_tags() {
    let a = mixed();
    let head = w::opus_head(2, 312, 0, None);
    // 200 samples short of the last packet's end: the stream ends inside it.
    let layout = OggLayout {
        end_trim: 200,
        packets_per_page: vec![4, 1, 7],
        ..OggLayout::default()
    };
    let source = file(&head, &a, &layout);
    let out = copy(&source);
    let (want, got) = (read(&source), read(&out));
    assert_eq!(want.packets[0].pts, -312);
    assert_eq!(want.stream.frames, Some((total(&a) - 312 - 200) as u64));
    assert_eq!(want.tags.len(), 3);
    assert_same(&got, &want);

    // The last granule is where the stream ends (pre-skip included), not where its last
    // packet does.
    let last = pages(&out).pop().unwrap();
    assert_eq!((last.flags, last.granule), (EOS, total(&a) - 200));
    // Writing is deterministic, and a copy of the copy is the copy.
    assert_eq!(copy(&out), out);
}

#[test]
fn a_stream_cut_at_the_front_or_shorter_than_its_pre_skip_reads_back_the_same() {
    let a = mixed();
    // RFC 7845 4.5: the first granule is past its packets; the copy starts at 0 again.
    let cut = file(
        &w::opus_head(2, 312, 0, None),
        &a,
        &OggLayout {
            start: 100_000,
            ..OggLayout::default()
        },
    );
    assert_same(&read(&copy(&cut)), &read(&cut));

    // Two 2.5 ms packets under a pre-skip of 3840: nothing is meant to be heard.
    let short = w::packets(16, false, 2, 20, 3);
    let source = file(
        &w::opus_head(1, 3840, 0, None),
        &short,
        &OggLayout::default(),
    );
    let got = read(&copy(&source));
    assert_eq!(got.stream.frames, Some(0));
    assert_eq!(got.packets.len(), 2);
    assert_same(&got, &read(&source));
}

#[test]
fn a_stream_of_unknown_length_is_written_whole() {
    let a = mixed();
    let source = file(
        &w::opus_head(2, 312, 0, None),
        &a,
        &OggLayout {
            end_trim: 200,
            ..OggLayout::default()
        },
    );
    // On a pipe the reader cannot see the last granule: no length, so nothing to trim to.
    let pipe = FaultySource::new(Cursor::new(source.clone())).not_seekable();
    let d = open_demuxer(Box::new(pipe), &Limits::default()).unwrap().1;
    assert_eq!(d.streams()[0].frames, None);
    let got = read(&copy_from(d));
    assert_eq!(got.stream.frames, Some((total(&a) - 312) as u64));
    assert_eq!(got.packets, read(&source).packets);
}

#[test]
fn a_stream_with_no_audio_is_two_headers_and_an_empty_last_page() {
    let head = w::opus_head(2, 312, 0, None);
    let source = file(&head, &[], &OggLayout::default());
    let out = copy(&source);
    let p = pages(&out);
    assert_eq!(p.len(), 3);
    assert_eq!((p[2].flags, p[2].granule), (EOS, 0));
    assert!(p[2].lacing.is_empty() && p[2].crc_ok);
    let got = read(&out);
    assert_eq!(got.stream.frames, Some(0));
    assert!(got.packets.is_empty());
    assert_same(&got, &read(&source));
}

#[test]
fn a_multistream_header_and_its_packets_pass_through_untouched() {
    // Family 1, three channels: a coupled stream and a mono one per packet.
    let head = w::opus_head(3, 312, 0, Some((1, 2, 1, &[0, 2, 1])));
    let a: Vec<Vec<u8>> = (0..12)
        .map(|i| {
            w::multistream_packet(&[
                w::packets(31, true, 1, 60, i).remove(0),
                w::packets(31, false, 1, 30, 100 + i).remove(0),
            ])
        })
        .collect();
    let source = file(&head, &a, &OggLayout::default());
    let out = copy(&source);
    assert_eq!(pages(&out)[0].body, head);
    let got = read(&out);
    assert_eq!(got.stream.extradata, head);
    assert_eq!(
        got.stream.audio.layout(),
        &ChannelLayout::vorbis_order(3).unwrap()
    );
    let data: Vec<Vec<u8>> = got.packets.iter().map(|p| p.data.clone()).collect();
    assert_eq!(data, a);
    assert_same(&got, &read(&source));
}

/// Audio packets ending on each audio page, and the page's granule.
fn audio_pages(file: &[u8]) -> Vec<(usize, i64)> {
    pages(file)[2..]
        .iter()
        .map(|p| (p.lacing.iter().filter(|&&l| l < 255).count(), p.granule))
        .collect()
}

#[test]
fn a_page_holds_at_most_a_second_of_whole_packets() {
    let head = w::opus_head(2, 312, 0, None);
    // 120 packets of 20 ms: 50 make a second exactly.
    let source = file(
        &head,
        &w::packets(31, true, 120, 30, 1),
        &OggLayout::default(),
    );
    let out = copy(&source);
    assert_eq!(
        audio_pages(&out),
        [(50, 48_000), (50, 96_000), (20, 115_200)]
    );
    assert_same(&read(&out), &read(&source));

    // 40 packets of 60 ms: a seventeenth would pass the second.
    let source = file(
        &head,
        &w::packets(3, true, 40, 30, 2),
        &OggLayout::default(),
    );
    let out = copy(&source);
    assert_eq!(
        audio_pages(&out),
        [(16, 46_080), (16, 92_160), (8, 115_200)]
    );
    let p = pages(&out);
    assert!(p[2..p.len() - 1].iter().all(|p| p.flags == 0));
    assert_eq!(p.last().unwrap().flags, EOS);

    // 300 packets of 2.5 ms are 0.75 s: the 255 lacing values come first.
    let source = file(
        &head,
        &w::packets(28, true, 300, 30, 3),
        &OggLayout::default(),
    );
    assert_eq!(audio_pages(&copy(&source)), [(255, 30_600), (45, 36_000)]);
}

#[test]
fn packets_past_the_end_stay_on_the_page_the_stream_ends_on() {
    let head = w::opus_head(2, 312, 0, None);
    let a = w::packets(31, true, 52, 30, 1);
    let source = file(&head, &a, &OggLayout::default());
    // The stream ends inside the 50th packet, or right after it: the granule may not go
    // back on a later page, so the two packets after it join its page.
    for frames in [47_000, 48_000 - 312] {
        let mut d = open(&source);
        let mut stream = d.streams()[0].clone();
        stream.frames = Some(frames);
        let (sink, mut m) = muxer(&stream, &Tags::new());
        let mut p = Packet::default();
        while d.read_packet(&mut p).unwrap() {
            m.write_packet(&p).unwrap();
        }
        m.finish().unwrap();
        let out = sink.contents();
        assert_eq!(audio_pages(&out), [(52, frames as i64 + 312)]);
        let got = read(&out);
        assert_eq!(got.stream.frames, Some(frames));
        assert_eq!(got.packets.len(), 52);
    }
}

/// A stereo stream with a pre-skip of 312 and no length, for packets written by hand.
fn bare_stream() -> StreamInfo {
    let mut s = StreamInfo::new(
        0,
        CodecId::Opus,
        AudioSpec::new(48_000, SampleFormat::F32, ChannelLayout::stereo()).unwrap(),
        TimeBase::new(1, 48_000).unwrap(),
    );
    s.extradata = w::opus_head(2, 312, 0, None);
    s
}

/// Writes `packets` as `(pts, data)`.
fn write(stream: &StreamInfo, packets: &[(i64, Vec<u8>)]) -> Vec<u8> {
    let (sink, mut m) = muxer(stream, &Tags::new());
    for (pts, data) in packets {
        let p = Packet {
            pts: *pts,
            duration: crossfade_opus::packet_duration(data).map_or(0, |n| n as u64),
            data: data.clone(),
            ..Packet::default()
        };
        m.write_packet(&p).unwrap();
    }
    m.finish().unwrap();
    sink.contents()
}

/// `(pts, data)` for packets that follow each other from `-312`.
fn in_a_row(audio: &[Vec<u8>]) -> Vec<(i64, Vec<u8>)> {
    let mut pts = -312;
    audio
        .iter()
        .map(|p| {
            let at = pts;
            pts += crossfade_opus::packet_duration(p).unwrap_or(0) as i64;
            (at, p.clone())
        })
        .collect()
}

#[test]
fn a_packet_over_255_lacing_values_continues_on_the_next_page() {
    let mut a = w::packets(31, true, 6, 30, 1);
    // 70,000 bytes = 274 * 255 + 130: more than the 65,025 a page can hold.
    a[3] = [vec![31 << 3 | 4], w::seeded_bytes(9, 69_999)].concat();
    // Nothing in a packet without a TOC to time it by.
    a.push(Vec::new());
    let out = write(&bare_stream(), &in_a_row(&a));
    let p = pages(&out);
    assert!(p.iter().all(|p| p.crc_ok));
    assert_eq!(
        audio_pages(&out),
        [(3, 2880), (0, -1), (4, 5760)],
        "the page that ends no packet has no granule"
    );
    assert_eq!(p[3].lacing, [255; 255]);
    assert_eq!(
        [p[2].flags, p[3].flags, p[4].flags],
        [0, 0, CONTINUED | EOS]
    );
    assert_eq!(p[4].lacing[..20], [[255; 19].as_slice(), &[130]].concat());

    let got = read(&out);
    let data: Vec<Vec<u8>> = got.packets.iter().map(|p| p.data.clone()).collect();
    assert!(data == a);
    let times: Vec<(i64, u64)> = got.packets.iter().map(|p| (p.pts, p.duration)).collect();
    let want: Vec<(i64, u64)> = (0..6).map(|i| (i * 960 - 312, 960)).collect();
    assert_eq!(times[..6], want);
    assert_eq!(times[6], (6 * 960 - 312, 0));
}

#[test]
fn a_hole_in_the_timeline_is_kept_as_a_jump_of_the_granule() {
    // Packets lost upstream: the fourth starts 4,800 samples late (RFC 7845 section 4).
    let a = w::packets(31, true, 5, 30, 1);
    let mut packets = in_a_row(&a);
    for (pts, _) in &mut packets[3..] {
        *pts += 4800;
    }
    let out = write(&bare_stream(), &packets);
    // The page closes at the hole, so the next page's granule places its packets after it.
    assert_eq!(audio_pages(&out), [(3, 2880), (2, 9600)]);
    let got = read(&out);
    let pts: Vec<i64> = got.packets.iter().map(|p| p.pts).collect();
    assert_eq!(pts, [-312, 648, 1608, 7368, 8328]);
    assert_eq!(got.stream.frames, Some(9600 - 312));
    assert_eq!(copy(&out), out);

    // Only a jump forward is one: the origin is the first packet's pts, and a pts that
    // goes back is not followed.
    let shifted: Vec<(i64, Vec<u8>)> = in_a_row(&a)
        .into_iter()
        .enumerate()
        .map(|(i, (pts, d))| (if i == 2 { 0 } else { pts + 1000 }, d))
        .collect();
    assert_eq!(
        write(&bare_stream(), &shifted),
        write(&bare_stream(), &in_a_row(&a))
    );
}

#[test]
fn the_identification_header_cannot_change_once_written() {
    let stream = bare_stream();
    let (sink, mut m) = muxer(&stream, &Tags::new());
    m.set_extradata(&stream.extradata).unwrap();
    assert_eq!(
        m.set_extradata(&w::opus_head(2, 400, 0, None)),
        Err(Error::Unsupported {
            what: "ogg opus: the identification header is already written and cannot change".into()
        })
    );
    m.finish().unwrap();
    assert_eq!(pages(&sink.contents())[0].body, stream.extradata);
}
