//! The Ogg Opus mapping (RFC 7845) on files built with `crossfade_opus::testing::ogg`: the
//! stream description, the timeline from granule positions (pre-skip, end trimming, a stream
//! cut at the front), tags, chains, refusals and seeking.

use crossfade_core::{
    ChannelLayout, CodecId, Demuxer, Error, Limits, Packet, SampleFormat, TimeBase,
};
use crossfade_format::{FormatId, open_demuxer};
use crossfade_opus::testing::ogg::{self as w, OggLayout};
use crossfade_testkit::io::FaultySource;
use std::io::Cursor;

fn open(bytes: &[u8]) -> Box<dyn Demuxer> {
    let (format, d) =
        open_demuxer(Box::new(Cursor::new(bytes.to_vec())), &Limits::default()).unwrap();
    assert_eq!(format, FormatId::Ogg);
    d
}

fn open_pipe(bytes: &[u8]) -> Box<dyn Demuxer> {
    let src = FaultySource::new(Cursor::new(bytes.to_vec())).not_seekable();
    open_demuxer(Box::new(src), &Limits::default()).unwrap().1
}

fn refused(bytes: Vec<u8>) -> Error {
    match open_demuxer(Box::new(Cursor::new(bytes)), &Limits::default()) {
        Err(e) => e,
        Ok(_) => panic!("opened"),
    }
}

fn all(d: &mut dyn Demuxer) -> Vec<Packet> {
    let mut out = vec![];
    let mut p = Packet::default();
    while d.read_packet(&mut p).unwrap() {
        out.push(p.clone());
    }
    out
}

/// A mix of SILK, hybrid and CELT packets of 2.5 to 60 ms.
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

fn durations(audio: &[Vec<u8>]) -> Vec<u64> {
    audio
        .iter()
        .map(|p| crossfade_opus::packet_duration(p).unwrap() as u64)
        .collect()
}

const TAGS: [&str; 3] = ["TITLE=Tone", "ARTIST=Nobody", "R128_TRACK_GAIN=-1024"];

fn file(pre_skip: u16, audio: &[Vec<u8>], layout: &OggLayout) -> Vec<u8> {
    w::ogg_file(
        &w::opus_head(2, pre_skip, 0, None),
        &w::opus_tags("crossfade test", &TAGS),
        audio,
        layout,
    )
}

#[test]
fn the_stream_is_described_from_its_headers() {
    let a = audio();
    let head = w::opus_head(2, 312, -256, None);
    let bytes = w::ogg_file(&head, &w::opus_tags("v", &TAGS), &a, &OggLayout::default());
    let d = open(&bytes);
    let s = &d.streams()[0];
    assert_eq!(s.codec, CodecId::Opus);
    assert_eq!(s.audio.format(), SampleFormat::F32);
    // Always 48 kHz: the header's input rate (44.1 kHz here) is information only.
    assert_eq!(s.audio.sample_rate(), 48_000);
    assert_eq!(s.audio.layout(), &ChannelLayout::stereo());
    assert_eq!(s.time_base, TimeBase::new(1, 48_000).unwrap());
    assert_eq!(s.extradata, head);
    let total: u64 = durations(&a).iter().sum();
    assert_eq!(s.frames, Some(total - 312));
    assert_eq!(s.bits_per_raw_sample, None);
    // Gain tags are carried, not applied.
    let tags: Vec<_> = d.tags().iter().map(|t| (&*t.key, &*t.value)).collect();
    assert_eq!(
        tags,
        [
            ("TITLE", "Tone"),
            ("ARTIST", "Nobody"),
            ("R128_TRACK_GAIN", "-1024")
        ]
    );
}

#[test]
fn the_layout_follows_the_mapping_family() {
    let a = w::packets(31, false, 2, 20, 1);
    let tags = w::opus_tags("v", &[]);
    let layout = |head: Vec<u8>| {
        open(&w::ogg_file(&head, &tags, &a, &OggLayout::default())).streams()[0]
            .audio
            .layout()
            .clone()
    };
    assert_eq!(layout(w::opus_head(1, 0, 0, None)), ChannelLayout::mono());
    assert_eq!(
        layout(w::opus_head(6, 0, 0, Some((1, 4, 2, &[0, 4, 1, 2, 3, 5])))),
        ChannelLayout::vorbis_order(6).unwrap()
    );
    assert_eq!(
        layout(w::opus_head(3, 0, 0, Some((255, 3, 0, &[0, 1, 2])))),
        ChannelLayout::unpositioned(3).unwrap()
    );
}

#[test]
fn packets_are_timed_from_the_granules_with_pre_skip_end_trim_and_a_cut_front() {
    let a = audio();
    let d = durations(&a);
    let total: i64 = d.iter().sum::<u64>() as i64;
    for per_page in [vec![1], vec![3, 7], vec![2, 1, 5]] {
        for (pre_skip, start, end_trim) in [
            (0u16, 0i64, 0i64),
            (312, 0, 0),
            (3840, 0, 517),
            (312, 96_000, 100),
            (0, 7, 0),
        ] {
            let layout = OggLayout {
                packets_per_page: per_page.clone(),
                start,
                end_trim,
                ..OggLayout::default()
            };
            let bytes = file(pre_skip, &a, &layout);
            let what = format!("{per_page:?} pre-skip {pre_skip} start {start} trim {end_trim}");
            // The first packet decodes the first pre-skip samples, whatever the start.
            let mut pos = -i64::from(pre_skip);
            let want: Vec<(i64, u64)> = d
                .iter()
                .map(|&n| {
                    let p = (pos, n);
                    pos += n as i64;
                    p
                })
                .collect();
            for mut dm in [open(&bytes), open_pipe(&bytes)] {
                let got: Vec<(i64, u64)> =
                    all(&mut *dm).iter().map(|p| (p.pts, p.duration)).collect();
                assert_eq!(got, want, "{what}");
            }
            let audible = total - i64::from(pre_skip) - end_trim;
            assert_eq!(
                open(&bytes).streams()[0].frames,
                Some(audible as u64),
                "{what}"
            );
            assert_eq!(open_pipe(&bytes).streams()[0].frames, None, "{what}");
        }
    }
}

#[test]
fn a_first_page_short_of_its_packets_is_refused_unless_it_is_the_last() {
    let a = w::packets(31, false, 6, 20, 2);
    // Two pages, the first's granule 100 short: invalid (RFC 7845 4.5).
    let bytes = file(
        0,
        &a,
        &OggLayout {
            packets_per_page: vec![3],
            start: -100,
            ..OggLayout::default()
        },
    );
    assert!(matches!(refused(bytes), Error::Malformed { .. }));
    // One page, the last: end trimming from 0.
    let one = OggLayout {
        packets_per_page: vec![6],
        end_trim: 1000,
        ..OggLayout::default()
    };
    for d in [open(&file(312, &a, &one)), open_pipe(&file(312, &a, &one))] {
        assert_eq!(d.streams()[0].frames, Some(6 * 960 - 1000 - 312));
    }
    let mut d = open(&file(312, &a, &one));
    assert_eq!(all(&mut *d)[0].pts, -312);
    // ... but not shorter than the pre-skip.
    let short = OggLayout {
        end_trim: 6 * 960 - 300,
        ..one
    };
    assert!(matches!(
        refused(file(312, &a, &short)),
        Error::Malformed { .. }
    ));
}

#[test]
fn a_chained_file_is_read_to_the_end_of_its_first_link() {
    let a = w::packets(31, false, 7, 20, 3);
    let b = w::packets(1, true, 4, 20, 4);
    let mut bytes = file(0, &a, &OggLayout::default());
    bytes.extend(file(
        0,
        &b,
        &OggLayout {
            serial: 99,
            ..OggLayout::default()
        },
    ));
    let mut d = open(&bytes);
    assert_eq!(d.streams()[0].frames, Some(7 * 960));
    assert_eq!(all(&mut *d).len(), 7);
}

#[test]
fn missing_and_broken_headers_are_refused() {
    let a = w::packets(31, false, 2, 20, 5);
    let tags = w::opus_tags("v", &[]);
    let malformed = |bytes: Vec<u8>| matches!(refused(bytes), Error::Malformed { .. });
    // A cut identification header, and one of a major version not 0.
    let head = w::opus_head(2, 0, 0, None);
    assert!(malformed(w::ogg_file(
        &head[..15],
        &tags,
        &a,
        &OggLayout::default()
    )));
    let mut v = head.clone();
    v[8] = 0x10;
    assert!(malformed(w::ogg_file(&v, &tags, &a, &OggLayout::default())));
    // No comment header: the stream ends, or an audio packet stands in its place.
    assert!(malformed(w::ogg_file(
        &head,
        &a[0],
        &a[1..],
        &OggLayout::default()
    )));
    let only_head = w::ogg_file(&head, &tags, &[], &OggLayout::default());
    let first_page = 28 + head.len();
    assert!(malformed(only_head[..first_page].to_vec()));
    // A comment header whose lengths run past it: the tags are dropped, the audio is read.
    let mut bad_tags = tags.clone();
    bad_tags[8] = 200;
    let mut d = open(&w::ogg_file(&head, &bad_tags, &a, &OggLayout::default()));
    assert!(d.tags().is_empty());
    assert_eq!(all(&mut *d).len(), 2);
}

#[test]
fn seeking_lands_80_ms_early_and_reads_on_in_order() {
    let mut a = vec![];
    for i in 0..40 {
        a.extend(w::packets([9, 15, 31, 28][i % 4], true, 3, 30, i as u64));
    }
    let d = durations(&a);
    for (pre_skip, start, per_page) in [(312u16, 0i64, vec![1]), (3840, 48_000, vec![4, 2])] {
        let layout = OggLayout {
            packets_per_page: per_page,
            start,
            end_trim: 200,
            ..OggLayout::default()
        };
        let bytes = file(pre_skip, &a, &layout);
        let mut dm = open(&bytes);
        let end = dm.streams()[0].frames.unwrap() as i64;
        let mut x = 7u64;
        let mut targets = vec![0, 1, 3839, 3840, 3841, 10_000, end - 1, end, end + 50, -20];
        for _ in 0..30 {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(7);
            targets.push((x >> 33) as i64 % end);
        }
        for t in targets {
            let landed = dm.seek(t).unwrap();
            let mut p = Packet::default();
            if t >= end {
                assert_eq!(landed, end, "target {t}");
                assert!(!dm.read_packet(&mut p).unwrap(), "target {t}");
                continue;
            }
            assert!(
                landed <= t.max(0) - 3840 || landed == -i64::from(pre_skip),
                "target {t}: {landed}"
            );
            assert!(dm.read_packet(&mut p).unwrap());
            assert_eq!(p.pts, landed, "target {t}");
            // The packets from there on are the file's, in order.
            let mut pos = -i64::from(pre_skip);
            let k = d
                .iter()
                .position(|&n| {
                    let here = pos;
                    pos += n as i64;
                    here == landed
                })
                .unwrap();
            assert_eq!(p.data, a[k], "target {t}");
            assert!(dm.read_packet(&mut p).unwrap());
            assert_eq!(p.data, a[k + 1], "target {t}");
        }
    }
    // A pipe cannot seek.
    let bytes = file(0, &a, &OggLayout::default());
    assert!(matches!(
        open_pipe(&bytes).seek(0),
        Err(Error::Unsupported { .. })
    ));
}
