//! The Ogg Vorbis mapping on streams built with `crossfade_vorbis::testing`: the stream
//! description, timestamps from granule positions (priming, a late start, end padding, bad
//! packets, pages of every size), chained files, refusals, and seeking.

use crossfade_codec::decoder_for;
use crossfade_core::{
    AudioFrame, ChannelLayout, CodecId, Demuxer, Error, Limits, Packet, SampleFormat, Samples,
    TimeBase,
};
use crossfade_format::{FormatId, open_demuxer};
use crossfade_testkit::io::FaultySource;
use crossfade_vorbis::testing::{self as w, OggLayout};
use std::io::Cursor;

use w::SampleStream as Built;

fn build(channels: u8, seq: &str, seed: u64) -> Built {
    w::sample_stream(channels, seq, seed)
}

fn file(b: &Built, layout: &OggLayout) -> Vec<u8> {
    w::ogg_file(
        [&b.headers[0], &b.headers[1], &b.headers[2]],
        &b.audio,
        layout,
    )
}

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

fn all(d: &mut dyn Demuxer) -> Vec<Packet> {
    let mut out = vec![];
    let mut p = Packet::default();
    while d.read_packet(&mut p).unwrap() {
        out.push(p.clone());
    }
    out
}

/// Each packet's expected (pts, duration): positions from `start`, shifted so that a late
/// start begins at 0.
fn expected(b: &Built, start: i64) -> Vec<(i64, u64)> {
    let ys = w::yields(&b.headers[0], &b.headers[2], &b.audio);
    let shift = start.max(0);
    let mut pos = start;
    ys.iter()
        .map(|&y| {
            let p = (pos - shift, y as u64);
            pos += y;
            p
        })
        .collect()
}

const SEQ: &str = "SSLLLSSLSLLLSSSSLLSLSSLLLLSLS";

#[test]
fn the_stream_is_described_from_its_headers() {
    let b = build(2, SEQ, 1);
    let bytes = file(&b, &OggLayout::default());
    let d = open(&bytes);
    let s = &d.streams()[0];
    assert_eq!(s.codec, CodecId::Vorbis);
    assert_eq!(s.audio.format(), SampleFormat::F32);
    assert_eq!(s.audio.sample_rate(), 32_000);
    assert_eq!(s.audio.layout(), &ChannelLayout::stereo());
    assert_eq!(s.time_base, TimeBase::new(1, 32_000).unwrap());
    assert_eq!(
        s.extradata,
        crossfade_vorbis::pack_headers([&b.headers[0], &b.headers[1], &b.headers[2]])
    );
    let total: i64 = w::yields(&b.headers[0], &b.headers[2], &b.audio)
        .iter()
        .sum();
    assert_eq!(s.frames, Some(total as u64));
    let tags: Vec<_> = d.tags().iter().map(|t| (&*t.key, &*t.value)).collect();
    assert_eq!(tags, [("TITLE", "Tone"), ("ARTIST", "Nobody")]);
    assert_eq!(s.bits_per_raw_sample, None);
}

#[test]
fn six_channels_get_the_specifications_layout() {
    let b = build(6, "SSL", 2);
    let d = open(&file(&b, &OggLayout::default()));
    assert_eq!(
        d.streams()[0].audio.layout(),
        &ChannelLayout::from_name("5.1-back").unwrap()
    );
    let b = build(9, "SSL", 2);
    let d = open(&file(&b, &OggLayout::default()));
    assert_eq!(
        d.streams()[0].audio.layout(),
        &ChannelLayout::unpositioned(9).unwrap()
    );
}

#[test]
fn packets_are_timed_from_the_granules_on_every_page_size() {
    let b = build(1, SEQ, 3);
    for per_page in [vec![1], vec![3, 7], vec![50], vec![2, 1, 5]] {
        for (start, end_trim) in [(0i64, 0i64), (-300, 0), (-1000, 77), (5000, 123), (0, 600)] {
            let layout = OggLayout {
                packets_per_page: per_page.clone(),
                start,
                end_trim,
                ..OggLayout::default()
            };
            let bytes = file(&b, &layout);
            let what = format!("{per_page:?} start {start} trim {end_trim}");
            // One page that is first and last has one granule for both ends: short of its
            // packets it is end padding, as libvorbisfile reads it (priming counts as padding
            // too); past them it is a late start (padding then only moves the start).
            let (start, end_trim) = match (per_page == [50], start) {
                (true, s) if s - end_trim < 0 => (0, end_trim - s),
                (true, s) => (s - end_trim, 0),
                _ => (start, end_trim),
            };
            let want = expected(&b, start);
            for mut d in [open(&bytes), open_pipe(&bytes)] {
                let got: Vec<(i64, u64)> =
                    all(&mut *d).iter().map(|p| (p.pts, p.duration)).collect();
                assert_eq!(got, want, "{what}");
            }
            let total: i64 = w::yields(&b.headers[0], &b.headers[2], &b.audio)
                .iter()
                .sum();
            let audible = start + total - end_trim - start.max(0);
            assert_eq!(
                open(&bytes).streams()[0].frames,
                Some(audible as u64),
                "{what}"
            );
            // A pipe cannot read ahead to the last granule: the padding is not trimmed.
            assert_eq!(open_pipe(&bytes).streams()[0].frames, None, "{what}");
        }
    }
}

/// Decodes every packet through the engine's adapter: frames (pts, interleaved samples).
fn decode(d: &mut dyn Demuxer) -> Vec<(i64, Vec<f32>)> {
    let mut dec = decoder_for(&d.streams()[0]).unwrap();
    let mut out = vec![];
    let mut f = AudioFrame::new(&d.streams()[0].audio);
    let mut p = Packet::default();
    while d.read_packet(&mut p).unwrap() {
        dec.send_packet(&p).unwrap();
        while dec.receive_frame(&mut f).unwrap() {
            let Samples::F32(s) = &f.samples else {
                panic!()
            };
            out.push((f.pts, s.clone()));
        }
    }
    out
}

#[test]
fn decoded_frames_follow_each_other_without_a_gap() {
    let b = build(2, SEQ, 4);
    let bytes = file(
        &b,
        &OggLayout {
            start: -700,
            packets_per_page: vec![3],
            ..OggLayout::default()
        },
    );
    let frames = decode(&mut *open(&bytes));
    let mut next = frames[0].0;
    assert!(next < 0, "priming comes out at negative pts");
    for (pts, s) in &frames {
        assert_eq!(*pts, next);
        next += (s.len() / 2) as i64;
    }
}

#[test]
fn a_packet_with_a_mode_that_does_not_exist_yields_nothing_and_the_rest_keep_their_places() {
    let b = build(1, "SSLXXXLSSXLLS", 5);
    let good = build(1, "SSLLSSLLS", 5);
    // The bad packets yield nothing, so the granules (counted as the decoder counts) and the
    // output skip them.
    let bytes = file(
        &b,
        &OggLayout {
            packets_per_page: vec![2],
            ..OggLayout::default()
        },
    );
    let got: Vec<(i64, u64)> = all(&mut *open(&bytes))
        .iter()
        .map(|p| (p.pts, p.duration))
        .collect();
    assert_eq!(got, expected(&b, 0));
    assert_eq!(got.iter().filter(|p| p.1 == 0).count(), 5);
    let frames = decode(&mut *open(&bytes));
    let good_frames = decode(&mut *open(&file(&good, &OggLayout::default())));
    assert_eq!(frames, good_frames);
}

#[test]
fn granules_that_count_skipped_packets_leave_a_gap_where_they_were() {
    // An encoder whose granules count the bad packets as if each had yielded 1000 frames: the
    // packets after them keep the positions their pages' granules give.
    let b = build(1, "SSLLXSSLL", 6);
    let mut ys = w::yields(&b.headers[0], &b.headers[2], &b.audio);
    ys[4] = 1000;
    let bytes = w::ogg_file_with_yields(
        [&b.headers[0], &b.headers[1], &b.headers[2]],
        &b.audio,
        &ys,
        &OggLayout {
            packets_per_page: vec![1],
            ..OggLayout::default()
        },
    );
    let got: Vec<i64> = all(&mut *open(&bytes)).iter().map(|p| p.pts).collect();
    let mut pos = 0;
    let want: Vec<i64> = ys
        .iter()
        .map(|&y| {
            let p = pos;
            pos += y;
            p
        })
        .collect();
    // The bad packet itself yields nothing and carries the next frame's pts.
    let mut want = want;
    want[4] = want[5];
    assert_eq!(got, want);
}

#[test]
fn a_chained_file_is_read_to_the_end_of_its_first_link() {
    let a = build(2, "SSLLS", 7);
    let b = build(1, "LLSSL", 8);
    let mut bytes = file(&a, &OggLayout::default());
    bytes.extend(file(
        &b,
        &OggLayout {
            serial: 99,
            ..OggLayout::default()
        },
    ));
    let d = open(&bytes);
    let total: i64 = w::yields(&a.headers[0], &a.headers[2], &a.audio)
        .iter()
        .sum();
    assert_eq!(d.streams()[0].frames, Some(total as u64));
    assert_eq!(d.streams()[0].audio.layout().channels(), 2);
    let mut d = d;
    assert_eq!(all(&mut *d).len(), a.audio.len());
}

#[test]
fn broken_headers_are_refused() {
    let b = build(2, "SSL", 9);
    // Granules from the good headers: the broken ones cannot count.
    let ys = w::yields(&b.headers[0], &b.headers[2], &b.audio);
    let malformed =
        |bytes: Vec<u8>| match open_demuxer(Box::new(Cursor::new(bytes)), &Limits::default()) {
            Err(Error::Malformed { what }) => what,
            Err(e) => panic!("{e:?}"),
            Ok(_) => panic!("opened"),
        };
    let swap = w::ogg_file_with_yields(
        [&b.headers[0], &b.headers[2], &b.headers[1]],
        &b.audio,
        &ys,
        &OggLayout::default(),
    );
    assert!(malformed(swap).contains("comment"));
    let mut bad_setup = b.headers[2].clone();
    bad_setup.truncate(bad_setup.len() / 2);
    let cut = w::ogg_file_with_yields(
        [&b.headers[0], &b.headers[1], &bad_setup],
        &b.audio,
        &ys,
        &OggLayout::default(),
    );
    assert!(malformed(cut).contains("setup"));
    let mut bad_ident = b.headers[0].clone();
    bad_ident[11] = 0;
    let zero = w::ogg_file_with_yields(
        [&bad_ident, &b.headers[1], &b.headers[2]],
        &b.audio,
        &ys,
        &OggLayout::default(),
    );
    assert!(malformed(zero).contains("channels"));
    // The identification header alone: the stream ends before the comment header.
    let only = w::ogg_file_with_yields(
        [&b.headers[0], &b.headers[1], &b.headers[2]],
        &[],
        &[],
        &OggLayout::default(),
    );
    let first_page = 27 + 1 + b.headers[0].len();
    let e = open_demuxer(
        Box::new(Cursor::new(only[..first_page].to_vec())),
        &Limits::default(),
    )
    .err();
    assert!(
        matches!(
            e,
            Some(Error::Malformed { .. } | Error::UnexpectedEof { .. })
        ),
        "{e:?}"
    );
    // The setup is parsed against the caller's allocation limit.
    let tight = Limits {
        max_alloc_bytes: 200,
        ..Limits::default()
    };
    let e = open_demuxer(
        Box::new(Cursor::new(file(&b, &OggLayout::default()))),
        &tight,
    )
    .err();
    assert!(matches!(e, Some(Error::AllocLimitExceeded { .. })), "{e:?}");
}

#[test]
fn a_stream_without_audio_packets_is_empty() {
    let b = build(2, "", 10);
    let mut d = open(&file(&b, &OggLayout::default()));
    assert_eq!(d.streams()[0].frames, Some(0));
    assert!(all(&mut *d).is_empty());
    assert_eq!(d.seek(10), Ok(0));
}

/// Seek-then-decode equals decode-then-slice, through the reader and the adapter (the graph's
/// session tests do the same through `DecodeSession`).
#[test]
fn seeking_lands_early_enough_for_the_overlap_and_decodes_the_same_bits() {
    let b = build(2, SEQ, 11);
    for layout in [
        OggLayout {
            packets_per_page: vec![1],
            start: -500,
            end_trim: 300,
            ..OggLayout::default()
        },
        OggLayout {
            packets_per_page: vec![4, 2],
            start: 3000,
            ..OggLayout::default()
        },
    ] {
        let bytes = file(&b, &layout);
        let whole: Vec<(i64, f32)> = decode(&mut *open(&bytes))
            .into_iter()
            .flat_map(|(pts, s)| {
                s.chunks(2)
                    .enumerate()
                    .map(move |(i, c)| (pts + i as i64, c[0]))
                    .collect::<Vec<_>>()
            })
            .collect();
        let end = open(&bytes).streams()[0].frames.unwrap() as i64;
        let mut d = open(&bytes);
        let mut x = 99u64;
        let mut targets = vec![0, 1, 255, 256, 2047, end - 1, end, end + 50, -20];
        for _ in 0..40 {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(7);
            targets.push((x >> 33) as i64 % end);
        }
        for t in targets {
            let landed = d.seek(t).unwrap();
            if t >= end {
                assert_eq!(landed, end);
                assert!(!d.read_packet(&mut Packet::default()).unwrap());
                continue;
            }
            assert!(
                landed <= t.max(0) || landed <= whole[0].0,
                "{t}: landed at {landed}"
            );
            let mut dec = decoder_for(&d.streams()[0]).unwrap();
            dec.reset();
            let mut got = vec![];
            let mut f = AudioFrame::new(&d.streams()[0].audio);
            let mut p = Packet::default();
            while d.read_packet(&mut p).unwrap() {
                dec.send_packet(&p).unwrap();
                while dec.receive_frame(&mut f).unwrap() {
                    let Samples::F32(s) = &f.samples else {
                        panic!()
                    };
                    got.extend(
                        s.chunks(2)
                            .enumerate()
                            .map(|(i, c)| (f.pts + i as i64, c[0])),
                    );
                }
            }
            let from = t.max(0);
            let got: Vec<_> = got.into_iter().filter(|&(pts, _)| pts >= from).collect();
            let want: Vec<_> = whole
                .iter()
                .copied()
                .filter(|&(pts, _)| pts >= from)
                .collect();
            assert!(
                got == want,
                "{layout:?} target {t}: {} vs {} frames",
                got.len(),
                want.len()
            );
        }
    }
}

#[test]
fn a_pipe_cannot_seek() {
    let b = build(1, "SSL", 12);
    let mut d = open_pipe(&file(&b, &OggLayout::default()));
    assert!(matches!(d.seek(0), Err(Error::Unsupported { .. })));
}

#[test]
fn a_file_cut_off_inside_its_last_packet_ends_with_the_last_whole_one() {
    let b = build(2, SEQ, 13);
    let bytes = file(
        &b,
        &OggLayout {
            packets_per_page: vec![1],
            ..OggLayout::default()
        },
    );
    let whole = all(&mut *open(&bytes));
    // Ten bytes into the last packet's page body.
    let last = b.audio.last().unwrap();
    let cut = bytes.len() - last.len() + 10;
    for mut d in [open(&bytes[..cut]), open_pipe(&bytes[..cut])] {
        assert_eq!(all(&mut *d), whole[..whole.len() - 1]);
    }
}
