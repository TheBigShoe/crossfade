//! The Ogg Opus writer (RFC 7845): files made with `crossfade_opus::testing::ogg` are read,
//! written again and read back, and the pages the writer produced are checked on their own.

mod seek_support;

use crossfade_core::{Demuxer, Limits, Packet, StreamInfo, Tags};
use crossfade_format::{FormatId, create_muxer, open_demuxer};
use crossfade_opus::testing::ogg::{self as w, OggLayout};
use seek_support::Shared;
use std::io::Cursor;

fn open(bytes: &[u8]) -> Box<dyn Demuxer> {
    open_demuxer(Box::new(Cursor::new(bytes.to_vec())), &Limits::default())
        .unwrap()
        .1
}

fn all(d: &mut dyn Demuxer) -> Vec<Packet> {
    let mut out = vec![];
    let mut p = Packet::default();
    while d.read_packet(&mut p).unwrap() {
        out.push(p.clone());
    }
    out
}

struct Read {
    info: StreamInfo,
    tags: Tags,
    packets: Vec<Packet>,
}

fn read(bytes: &[u8]) -> Read {
    let mut d = open(bytes);
    Read {
        info: d.streams()[0].clone(),
        tags: d.tags().clone(),
        packets: all(d.as_mut()),
    }
}

/// Stream copy: what a reader hands out goes to the writer unchanged.
fn copy(bytes: &[u8]) -> Vec<u8> {
    let mut d = open(bytes);
    let sink = Shared::default();
    let mut m = create_muxer(
        FormatId::Ogg,
        Box::new(sink.clone()),
        &d.streams()[0],
        d.tags(),
    )
    .unwrap();
    for p in all(d.as_mut()) {
        m.write_packet(&p).unwrap();
    }
    m.finish().unwrap();
    sink.bytes()
}

fn audio() -> Vec<Vec<u8>> {
    let mut a = vec![];
    for (i, &(config, n)) in [(9, 3), (3, 2), (15, 4), (16, 3), (29, 2), (31, 5), (1, 2)]
        .iter()
        .enumerate()
    {
        a.extend(w::packets(config, true, n, 40, i as u64));
    }
    a
}

const TAGS: [&str; 3] = ["TITLE=Tone", "ARTIST=Nobody", "R128_TRACK_GAIN=-1024"];

fn file(pre_skip: u16, audio: &[Vec<u8>], layout: &OggLayout) -> Vec<u8> {
    w::ogg_file(
        &w::opus_head(2, pre_skip, 0, None),
        &w::opus_tags("source", &TAGS),
        audio,
        layout,
    )
}

fn assert_same(a: &Read, b: &Read) {
    assert_eq!(a.info, b.info);
    assert_eq!(a.tags, b.tags);
    assert_eq!(a.packets, b.packets);
}

#[test]
fn a_stream_copy_reads_back_the_same() {
    let src = file(312, &audio(), &OggLayout::default());
    let written = copy(&src);
    assert_same(&read(&src), &read(&written));
}

// ---- the pages, parsed independently of the reader ----

struct Page {
    flags: u8,
    granule: i64,
    sequence: u32,
    serial: u32,
    lacing: Vec<u8>,
    body: Vec<u8>,
    crc_ok: bool,
}

fn pages(bytes: &[u8]) -> Vec<Page> {
    let mut out = vec![];
    let mut at = 0;
    while at < bytes.len() {
        let h = &bytes[at..];
        assert_eq!(&h[..4], b"OggS");
        let n = usize::from(h[26]);
        let lacing = h[27..27 + n].to_vec();
        let len: usize = lacing.iter().map(|&l| usize::from(l)).sum();
        let mut page = h[..27 + n + len].to_vec();
        let stored = u32::from_le_bytes(page[22..26].try_into().unwrap());
        page[22..26].fill(0);
        out.push(Page {
            flags: h[5],
            granule: i64::from_le_bytes(h[6..14].try_into().unwrap()),
            serial: u32::from_le_bytes(h[14..18].try_into().unwrap()),
            sequence: u32::from_le_bytes(h[18..22].try_into().unwrap()),
            lacing,
            body: h[27 + n..27 + n + len].to_vec(),
            crc_ok: w::ogg_crc(&page) == stored,
        });
        at += 27 + n + len;
    }
    out
}

/// Packets that end on a page.
fn ends(p: &Page) -> usize {
    p.lacing.iter().filter(|&&l| l < 255).count()
}

fn dur(p: &[u8]) -> i64 {
    crossfade_opus::packet_duration(p).unwrap() as i64
}

#[test]
fn the_pages_are_valid_and_carry_the_specified_granules() {
    let a = audio();
    let layout = OggLayout {
        packets_per_page: vec![3],
        end_trim: 200,
        ..OggLayout::default()
    };
    let out = copy(&file(312, &a, &layout));
    let ps = pages(&out);
    assert!(ps.iter().all(|p| p.crc_ok && p.serial == ps[0].serial));
    for (i, p) in ps.iter().enumerate() {
        assert_eq!(p.sequence, i as u32);
        assert_eq!(p.flags & 0x02 != 0, i == 0, "BOS on page {i}");
        assert_eq!(p.flags & 0x04 != 0, i == ps.len() - 1, "EOS on page {i}");
        assert_eq!(p.flags & 0x01, 0, "no packet continues over a page here");
    }
    // The head alone on page 0, the tags after it, both with granule 0.
    assert_eq!(ps[0].body, w::opus_head(2, 312, 0, None));
    assert_eq!(ps[0].lacing.len(), 1);
    assert!(ps[1].body.starts_with(b"OpusTags"));
    assert_eq!((ps[0].granule, ps[1].granule), (0, 0));
    let n = u32::from_le_bytes(ps[1].body[8..12].try_into().unwrap()) as usize;
    let vendor = std::str::from_utf8(&ps[1].body[12..12 + n]).unwrap();
    assert!(vendor.starts_with("crossfade "), "{vendor}");
    // A page's granule counts the samples through the last packet ending on it, pre-skip
    // included; the last page is cut by the end trimming.
    let total: i64 = a.iter().map(|p| dur(p)).sum();
    let mut left = a.iter().map(|p| dur(p));
    let mut done = 0;
    for (i, p) in ps[2..].iter().enumerate() {
        for _ in 0..ends(p) {
            done += left.next().unwrap();
        }
        let last = i == ps.len() - 3;
        assert_eq!(
            p.granule,
            if last { total - 200 } else { done },
            "page {}",
            i + 2
        );
    }
    assert_eq!(done, total);
    assert_eq!(read(&out).info.frames, Some((total - 200 - 312) as u64));
}

#[test]
fn a_page_holds_at_most_a_second_and_big_packets_are_laced_over_pages() {
    // 300 packets of 20 ms: 6 s. And a few packets of 2000 bytes (8 lacing values each).
    let mut a = w::packets(9, true, 300, 3, 1);
    a.extend(w::packets(9, true, 12, 2000, 2));
    let out = copy(&file(
        312,
        &a,
        &OggLayout {
            packets_per_page: vec![50],
            ..OggLayout::default()
        },
    ));
    let ps = pages(&out);
    let mut prev = 0;
    for p in &ps[2..] {
        assert!(p.granule - prev <= 48_000, "{} to {}", prev, p.granule);
        prev = p.granule;
    }
    assert!(ps.len() > 8);
    assert_same(&read(&file(312, &a, &OggLayout::default())), &read(&out));
}

#[test]
fn a_page_that_ends_no_packet_has_granule_minus_one() {
    // One packet of 70000 bytes needs 275 lacing values: it spans two pages.
    let p = vec![pkt(0, 960, &w::packets(9, true, 1, 70_000, 4)[0])];
    let out = encode(&info(0, None), &p).unwrap();
    let ps = pages(&out);
    assert_eq!(ps.len(), 4);
    assert_eq!(ps[2].granule, -1);
    assert_eq!(ps[3].granule, 960);
    assert_eq!(ps[3].flags & 1, 1, "continued");
    assert_eq!(read(&out).packets, p);
}

// ---- stream copy on a matrix of files ----

#[test]
fn stream_copy_is_the_identity_across_heads_trims_and_page_layouts() {
    let table: [u8; 6] = [0, 4, 1, 2, 3, 5];
    let heads = [
        w::opus_head(2, 0, 0, None),
        w::opus_head(2, 312, -256, None),
        w::opus_head(1, 3840, 0, None),
        w::opus_head(6, 312, 0, Some((1, 4, 2, &table))),
        w::opus_head(3, 100, 0, Some((255, 3, 0, &[0, 1, 2]))),
    ];
    let layouts = [
        OggLayout::default(),
        OggLayout {
            packets_per_page: vec![1],
            ..OggLayout::default()
        },
        OggLayout {
            packets_per_page: vec![3, 1, 7],
            end_trim: 700,
            ..OggLayout::default()
        },
        OggLayout {
            start: 5_000,
            end_trim: 1,
            ..OggLayout::default()
        },
    ];
    for head in &heads {
        for layout in &layouts {
            let a = audio();
            let src = w::ogg_file(head, &w::opus_tags("source", &TAGS), &a, layout);
            let written = copy(&src);
            let (r, back) = (read(&src), read(&written));
            assert_same(&r, &back);
            assert_eq!(r.info.extradata, *head);
            // Written again, it is the same bytes: the writer is deterministic.
            assert_eq!(copy(&written), written);
        }
    }
}

#[test]
fn tags_are_written_as_vorbis_comments_and_read_back() {
    let mut tags = Tags::new();
    for i in 0..400 {
        tags.push(format!("KEY{i}"), format!("värde {i} {}", "x".repeat(200)));
    }
    tags.push("TITLE", "Tone");
    let a = audio();
    let head = w::opus_head(2, 312, 0, None);
    let src = w::ogg_file(&head, &w::opus_tags("v", &[]), &a, &OggLayout::default());
    let mut d = open(&src);
    let sink = Shared::default();
    let mut m = create_muxer(
        FormatId::Ogg,
        Box::new(sink.clone()),
        &d.streams()[0],
        &tags,
    )
    .unwrap();
    for p in all(d.as_mut()) {
        m.write_packet(&p).unwrap();
    }
    m.finish().unwrap();
    let out = sink.bytes();
    assert_eq!(read(&out).tags, tags);
    // The tags span pages; audio starts on a fresh one.
    let ps = pages(&out);
    assert!(ps[1].lacing.len() == 255 && ps[2].flags & 1 == 1);
    let first_audio = ps.iter().position(|p| p.body.starts_with(&a[0])).unwrap();
    assert_eq!(ps[first_audio].flags & 1, 0);
    assert_eq!(read(&out).packets.len(), a.len());
}

// ---- an encoder's stream: no reader behind it ----

fn info(pre_skip: u16, frames: Option<u64>) -> StreamInfo {
    use crossfade_core::{AudioSpec, ChannelLayout, CodecId, SampleFormat, TimeBase};
    let audio = AudioSpec::new(48_000, SampleFormat::F32, ChannelLayout::stereo()).unwrap();
    let mut s = StreamInfo::new(0, CodecId::Opus, audio, TimeBase::new(1, 48_000).unwrap());
    s.extradata = w::opus_head(2, pre_skip, 0, None);
    s.frames = frames;
    s
}

fn pkt(pts: i64, duration: u64, data: &[u8]) -> Packet {
    Packet {
        stream: 0,
        pts,
        duration,
        data: data.to_vec(),
    }
}

fn encode(stream: &StreamInfo, packets: &[Packet]) -> Result<Vec<u8>, crossfade_core::Error> {
    let sink = Shared::default();
    let mut m = create_muxer(FormatId::Ogg, Box::new(sink.clone()), stream, &Tags::new())?;
    for p in packets {
        m.write_packet(p)?;
    }
    m.finish()?;
    Ok(sink.bytes())
}

/// 20 ms CELT-less SILK packets starting at `-pre_skip`.
fn timeline(pre_skip: i64, n: usize) -> Vec<Packet> {
    w::packets(9, true, n, 30, 7)
        .iter()
        .enumerate()
        .map(|(i, d)| pkt(-pre_skip + 960 * i as i64, 960, d))
        .collect()
}

#[test]
fn the_last_packet_may_state_a_shorter_duration_and_that_ends_the_audio() {
    let mut p = timeline(312, 5);
    p[4].duration = 500;
    let out = encode(&info(312, None), &p).unwrap();
    let r = read(&out);
    // Audible: 4 * 960 - 312 + 500.
    assert_eq!(r.info.frames, Some(4 * 960 - 312 + 500));
    assert_eq!(pages(&out).last().unwrap().granule, 4 * 960 + 500);
    // The reader hands out packets whole.
    assert_eq!(r.packets.len(), 5);
    assert_eq!(r.packets[4].duration, 960);
}

#[test]
fn stream_info_frames_ends_the_audio_too() {
    let p = timeline(312, 5);
    let out = encode(&info(312, Some(4000)), &p).unwrap();
    assert_eq!(read(&out).info.frames, Some(4000));
    assert_eq!(pages(&out).last().unwrap().granule, 4312);
    // Frames past the packets' end change nothing: the file holds what it holds.
    let out = encode(&info(312, Some(1_000_000)), &p).unwrap();
    assert_eq!(read(&out).info.frames, Some(5 * 960 - 312));
}

#[test]
fn nonsense_is_refused_rather_than_written() {
    let ok = timeline(312, 4);
    let bad = |f: &dyn Fn(&mut Vec<Packet>), stream: StreamInfo| {
        let mut p = ok.clone();
        f(&mut p);
        encode(&stream, &p).unwrap_err().to_string()
    };
    let s = || info(312, None);
    assert!(bad(&|p| p[0].pts = 0, s()).contains("expected"));
    assert!(bad(&|p| p[2].pts += 1, s()).contains("expected"));
    assert!(bad(&|p| p[1].duration = 961, s()).contains("duration"));
    assert!(bad(&|p| p[1].duration = 0, s()).contains("duration"));
    // Short only at the end.
    assert!(bad(&|p| p[1].duration = 500, s()).contains("last packet"));
    assert!(bad(&|p| p[1].data.clear(), s()).contains("ogg opus"));
    // A length that ends before the last packet starts.
    assert!(bad(&|_| {}, info(312, Some(10))).contains("length"));
    let mut wrong = info(312, None);
    wrong.extradata = b"OpusHeax".to_vec();
    assert!(bad(&|_| {}, wrong).contains("ogg opus"));
    let mut mono = info(312, None);
    mono.extradata = w::opus_head(1, 312, 0, None);
    assert!(bad(&|_| {}, mono).contains("channels"));
}

#[test]
fn only_48_khz_is_written() {
    use crossfade_core::{AudioSpec, ChannelLayout, SampleFormat};
    let mut s = info(312, None);
    s.audio = AudioSpec::new(44_100, SampleFormat::F32, ChannelLayout::stereo()).unwrap();
    let e = create_muxer(FormatId::Ogg, Box::new(Shared::default()), &s, &Tags::new());
    assert!(e.err().unwrap().to_string().contains("48 kHz"));
}

#[test]
fn a_short_file_and_an_empty_stream_are_valid() {
    // One packet, fewer audible frames than the pre-skip leaves of it.
    for frames in [0u64, 1, 100, 648] {
        let p = timeline(312, 1);
        let out = encode(&info(312, Some(frames)), &p).unwrap();
        let r = read(&out);
        assert_eq!(r.info.frames, Some(frames));
        assert_eq!(r.packets, p);
        assert_eq!(pages(&out).last().unwrap().granule, 312 + frames as i64);
    }
    // A packet that lies wholly in the priming.
    let mut p = timeline(2000, 1);
    p[0].duration = 960;
    let out = encode(&info(2000, None), &p).unwrap();
    assert_eq!(read(&out).info.frames, Some(0));
    assert_eq!(pages(&out).last().unwrap().granule, 2000);
}

#[test]
fn an_empty_stream_is_the_two_header_pages() {
    let out = encode(&info(312, None), &[]).unwrap();
    let ps = pages(&out);
    assert_eq!(ps.len(), 2);
    assert_eq!(ps[0].flags, 0x02);
    assert_eq!(ps[1].flags, 0x04);
    assert!(ps.iter().all(|p| p.crc_ok && p.granule == 0));
    let r = read(&out);
    assert_eq!((r.info.frames, r.packets.len()), (Some(0), 0));
}

// ---- the head after the first page ----

#[test]
fn a_final_head_may_change_what_the_pages_do_not_depend_on() {
    let p = timeline(312, 3);
    let run = |sink: Box<dyn crossfade_core::MediaSink>, head: &[u8]| {
        let mut m = create_muxer(FormatId::Ogg, sink, &info(312, None), &Tags::new()).unwrap();
        for x in &p {
            m.write_packet(x).unwrap();
        }
        let r = m.set_extradata(head);
        (m, r)
    };
    // The same head, and one with another gain and input rate: patched in place.
    let same = w::opus_head(2, 312, 0, None);
    let sink = Shared::default();
    let (mut m, r) = run(Box::new(sink.clone()), &same);
    r.unwrap();
    m.finish().unwrap();
    let plain = sink.bytes();
    let gain = w::opus_head(2, 312, 512, None);
    let sink = Shared::default();
    let (mut m, r) = run(Box::new(sink.clone()), &gain);
    r.unwrap();
    m.finish().unwrap();
    let patched = sink.bytes();
    assert_eq!(patched.len(), plain.len());
    assert_eq!(read(&patched).info.extradata, gain);
    assert_eq!(read(&patched).packets, read(&plain).packets);
    assert!(pages(&patched).iter().all(|p| p.crc_ok));
    // On a pipe the provisional head stays.
    let sink = Shared::default();
    let (mut m, r) = run(Box::new(Pipe(sink.clone())), &gain);
    r.unwrap();
    m.finish().unwrap();
    assert_eq!(sink.bytes(), plain);
    // The pre-skip and the channels are what the pages were written against.
    for head in [w::opus_head(2, 313, 0, None), w::opus_head(1, 312, 0, None)] {
        let (_, r) = run(Box::new(Shared::default()), &head);
        assert!(r.unwrap_err().to_string().contains("pre-skip"));
    }
}

struct Pipe(Shared);

impl crossfade_core::MediaSink for Pipe {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.0.write_all(buf)
    }
    fn seek(&mut self, _: std::io::SeekFrom) -> std::io::Result<u64> {
        Err(std::io::ErrorKind::NotSeekable.into())
    }
    fn finish(&mut self) -> std::io::Result<()> {
        self.0.finish()
    }
}

// ---- seeking in what was written ----

#[test]
fn a_written_file_seeks_like_its_source() {
    let a = w::packets(9, true, 400, 25, 3);
    let src = file(
        312,
        &a,
        &OggLayout {
            packets_per_page: vec![20],
            end_trim: 300,
            ..OggLayout::default()
        },
    );
    let out = copy(&src);
    let everything = read(&out).packets;
    for target in [0i64, 1, 47_999, 100_000, 7_000_000] {
        let mut d = open(&out);
        let at = d.seek(target).unwrap();
        assert!(at <= target.max(0));
        let rest = all(d.as_mut());
        let from = everything
            .iter()
            .position(|p| p.pts == at)
            .unwrap_or(everything.len());
        assert_eq!(rest, everything[from..], "seek to {target}");
        if target < 300_000 {
            assert!(
                at >= target.saturating_sub(48_000 + 960 * 20 + 960),
                "{at} for {target}"
            );
        }
    }
}
