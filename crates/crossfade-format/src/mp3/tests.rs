use super::*;
use crate::open_demuxer;
use crossfade_core::tag_keys;
use crossfade_testkit::io::FaultySource;
use std::io::Cursor;

/// MPEG-1 Layer III, 128 kbit/s, 44.1 kHz, joint stereo: 417 bytes, 418 padded.
const H128: [u8; 4] = [0xFF, 0xFB, 0x90, 0x64];
const H128_MONO: [u8; 4] = [0xFF, 0xFB, 0x90, 0xC4];
/// MPEG-2 Layer III, 64 kbit/s, 24 kHz, stereo: 192 bytes.
const H_LSF: [u8; 4] = [0xFF, 0xF3, 0x84, 0x00];

/// A frame of the header's length whose body is `fill` (never 0xFF, so never a sync word).
fn frame(h: [u8; 4], fill: u8) -> Vec<u8> {
    let len = FrameHeader::parse(&h).unwrap().frame_len().unwrap();
    let mut f = vec![fill; len];
    f[..4].copy_from_slice(&h);
    f
}

fn frames(h: [u8; 4], n: usize) -> Vec<u8> {
    (0..n).flat_map(|i| frame(h, i as u8 % 200)).collect()
}

/// LAME's first frame with `frames`, `delay` and `padding`.
fn lame_frame(frames: u32, delay: u16, padding: u16) -> Vec<u8> {
    let mut f = frame(H128, 0);
    let mut t = b"Info".to_vec();
    t.extend(3u32.to_be_bytes());
    t.extend(frames.to_be_bytes());
    t.extend(0u32.to_be_bytes());
    t.extend(b"LAME3.100");
    t.extend([0u8; 12]);
    t.extend([
        (delay >> 4) as u8,
        ((delay & 15) << 4) as u8 | (padding >> 8) as u8,
        padding as u8,
    ]);
    f[36..36 + t.len()].copy_from_slice(&t);
    f
}

fn seekable(bytes: &[u8]) -> Box<dyn MediaSource> {
    Box::new(Cursor::new(bytes.to_vec()))
}

fn pipe(bytes: &[u8]) -> Box<dyn MediaSource> {
    Box::new(FaultySource::new(Cursor::new(bytes.to_vec())).not_seekable())
}

fn read_all(d: &mut dyn Demuxer) -> Result<Vec<Packet>, Error> {
    let mut out = Vec::new();
    let mut p = Packet::default();
    while d.read_packet(&mut p)? {
        out.push(p.clone());
    }
    Ok(out)
}

fn open_bytes(src: Box<dyn MediaSource>) -> Result<Box<dyn Demuxer>, Error> {
    open(src, None, &Limits::default())
}

#[test]
fn a_cbr_stream_gives_one_packet_per_frame_with_the_decoders_delay() {
    let bytes = frames(H128, 10);
    for src in [seekable(&bytes), pipe(&bytes)] {
        let is_pipe = !src.is_seekable();
        let mut d = open_bytes(src).unwrap();
        let s = &d.streams()[0];
        assert_eq!(
            (s.codec, s.audio.sample_rate(), s.audio.layout().channels()),
            (CodecId::Mp3, 44100, 2)
        );
        assert_eq!(
            (s.audio.format(), s.time_base),
            (SampleFormat::F32, TimeBase::new(1, 44100).unwrap())
        );
        assert!(s.extradata.is_empty());
        // Only a seekable input can count its frames without an info frame.
        assert_eq!(s.frames, if is_pipe { None } else { Some(10 * 1152 - 529) });
        let packets = read_all(&mut *d).unwrap();
        assert_eq!(packets.len(), 10);
        for (i, p) in packets.iter().enumerate() {
            assert_eq!((p.pts, p.duration), (-529 + 1152 * i as i64, 1152));
            assert_eq!(p.data, frame(H128, i as u8));
        }
    }
}

#[test]
fn the_lame_info_frame_is_not_audio_and_sets_the_timeline() {
    let mut bytes = lame_frame(5, 576, 1404);
    bytes.extend(frames(H128, 5));
    for src in [seekable(&bytes), pipe(&bytes)] {
        let mut d = open_bytes(src).unwrap();
        assert_eq!(d.streams()[0].frames, Some(5 * 1152 - 576 - 1404));
        let packets = read_all(&mut *d).unwrap();
        assert_eq!(packets.len(), 5);
        assert_eq!(packets[0].pts, -(576 + 529));
        assert_eq!(packets[0].data, frame(H128, 0));
    }
}

#[test]
fn leading_zeros_and_junk_with_false_syncs_are_skipped() {
    for junk in [
        vec![0u8; 215],
        // A lone valid-looking header whose frame is not followed by another.
        [&H128[..], &[0x11; 30], &[0xFF, 0xFB], &[0xFF; 5]].concat(),
        b"some leading text".to_vec(),
    ] {
        let bytes = [&junk[..], &frames(H128, 3)].concat();
        let mut d = open_bytes(pipe(&bytes)).unwrap();
        let packets = read_all(&mut *d).unwrap();
        assert_eq!(packets.len(), 3, "{junk:02X?}");
        assert_eq!(packets[0].data, frame(H128, 0));
    }
}

#[test]
fn leading_junk_is_bounded() {
    let bytes = [
        &vec![0u8; MAX_LEADING_JUNK as usize + 10][..],
        &frames(H128, 3),
    ]
    .concat();
    let e = open_bytes(pipe(&bytes)).err().unwrap();
    assert!(
        matches!(e, Error::Malformed { ref what } if what.contains("no MPEG audio frame")),
        "{e:?}"
    );
}

#[test]
fn garbage_in_mid_stream_is_skipped_and_reading_resumes_at_a_confirmed_frame() {
    let bytes = [
        &frames(H128, 2)[..],
        &[0xFF, 0xFB, 0x90, 0x64, 1, 2, 3][..],
        &[0x55; 300][..],
        &frames(H128, 3)[..],
    ]
    .concat();
    let mut d = open_bytes(pipe(&bytes)).unwrap();
    let packets = read_all(&mut *d).unwrap();
    assert_eq!(packets.len(), 5);
    assert_eq!(packets[2].data, frame(H128, 0));
    assert_eq!(packets[4].pts, -529 + 4 * 1152);
}

#[test]
fn a_truncated_last_frame_is_dropped() {
    let mut bytes = frames(H128, 4);
    bytes.truncate(bytes.len() - 10);
    for src in [seekable(&bytes), pipe(&bytes)] {
        let mut d = open_bytes(src).unwrap();
        assert_eq!(read_all(&mut *d).unwrap().len(), 3);
    }
}

fn id3v1(title: &str, track: u8, genre: u8) -> Vec<u8> {
    let mut t = vec![0u8; 128];
    t[..3].copy_from_slice(b"TAG");
    t[3..3 + title.len()].copy_from_slice(title.as_bytes());
    t[33..39].copy_from_slice(b"Artist");
    t[93..97].copy_from_slice(b"1999");
    t[97..104].copy_from_slice(b"comment");
    t[126] = track;
    t[127] = genre;
    t
}

fn ape(items: &[u8]) -> Vec<u8> {
    let size = (items.len() + 32) as u32;
    let mut header = b"APETAGEX".to_vec();
    header.extend(2000u32.to_le_bytes());
    header.extend(size.to_le_bytes());
    header.extend(1u32.to_le_bytes());
    header.extend(0xA000_0000u32.to_le_bytes());
    header.extend([0u8; 8]);
    let mut footer = header.clone();
    footer[20..24].copy_from_slice(&0x8000_0000u32.to_le_bytes());
    [header, items.to_vec(), footer].concat()
}

#[test]
fn trailing_ape_and_id3v1_tags_are_not_audio() {
    // The APE tag's items include a header-shaped run that would pass as a frame.
    let fake = frames(H128, 2);
    let bytes = [frames(H128, 3), ape(&fake), id3v1("Title", 7, 17)].concat();
    let mut d = open_bytes(seekable(&bytes)).unwrap();
    assert_eq!(d.streams()[0].frames, Some(3 * 1152 - 529));
    assert_eq!(d.tags().get(tag_keys::TITLE), Some("Title"));
    assert_eq!(d.tags().get(tag_keys::TRACKNUMBER), Some("7"));
    assert_eq!(d.tags().get(tag_keys::GENRE), Some("Rock"));
    assert_eq!(d.tags().get(tag_keys::DATE), Some("1999"));
    assert_eq!(d.tags().get(tag_keys::COMMENT), Some("comment"));
    assert_eq!(read_all(&mut *d).unwrap().len(), 3);
    // On a pipe an ID3v1 tag simply fails to parse as frames.
    let bytes = [frames(H128, 3), id3v1("Title", 7, 17)].concat();
    let mut d = open_bytes(pipe(&bytes)).unwrap();
    assert!(d.tags().is_empty());
    assert_eq!(read_all(&mut *d).unwrap().len(), 3);
}

#[test]
fn free_format_length_is_found_once_and_reused() {
    // Free format, 44.1 kHz stereo; frames of 600 bytes, every third one padded.
    let make = |padded: bool, fill: u8| {
        let mut f = vec![fill; if padded { 601 } else { 600 }];
        f[..4].copy_from_slice(&[0xFF, 0xFB, if padded { 0x02 } else { 0x00 }, 0x64]);
        f
    };
    let bytes: Vec<u8> = (0..9).flat_map(|i| make(i % 3 == 2, i as u8)).collect();
    for src in [seekable(&bytes), pipe(&bytes)] {
        let mut d = open_bytes(src).unwrap();
        let packets = read_all(&mut *d).unwrap();
        let lens: Vec<usize> = packets.iter().map(|p| p.data.len()).collect();
        assert_eq!(lens, [600, 600, 601, 600, 600, 601, 600, 600, 601]);
    }
}

#[test]
fn layer_one_and_two_are_named_and_refused() {
    let l2 = [0xFF, 0xFD, 0x84, 0x00];
    let bytes = frames(l2, 3);
    match open_bytes(pipe(&bytes)).err().unwrap() {
        Error::Unsupported { what } => {
            assert_eq!(
                what,
                "mp3: MPEG-1 layer II audio is not supported, only layer III"
            );
        }
        e => panic!("{e:?}"),
    }
}

#[test]
fn a_change_of_channels_or_rate_is_refused_where_it_happens() {
    let bytes = [frames(H128, 2), frames(H128_MONO, 2)].concat();
    let mut d = open_bytes(pipe(&bytes)).unwrap();
    let e = read_all(&mut *d).err().unwrap();
    assert_eq!(
        e,
        Error::Unsupported {
            what: "mp3: the channel count changes at byte 834, which this reader cannot follow"
                .into()
        }
    );
    let h48 = [0xFF, 0xFB, 0x94, 0x64];
    let bytes = [frames(H128, 2), frames(h48, 3)].concat();
    let mut d = open_bytes(pipe(&bytes)).unwrap();
    let e = read_all(&mut *d).err().unwrap();
    assert_eq!(
        e,
        Error::Unsupported {
            what: "mp3: the sample rate changes at byte 834, which this reader cannot follow"
                .into()
        }
    );
}

#[test]
fn routed_through_open_demuxer_with_its_id3_tag() {
    let mut tag = b"ID3\x03\x00\x00\x00\x00\x00\x15".to_vec();
    tag.extend(b"TIT2\x00\x00\x00\x0B\x00\x00\x00Some title");
    let bytes = [tag, frames(H128, 2)].concat();
    let (format, mut d) = open_demuxer(seekable(&bytes), &Limits::default()).unwrap();
    assert_eq!(format, crate::FormatId::Mp3);
    assert_eq!(d.tags().get(tag_keys::TITLE), Some("Some title"));
    assert_eq!(d.streams()[0].frames, Some(2 * 1152 - 529));
    assert_eq!(read_all(&mut *d).unwrap().len(), 2);
}

/// A stream whose frames' main data sizes differ, with one frame of each kind per index.
fn lsf_stream(n: usize) -> Vec<u8> {
    frames(H_LSF, n)
}

#[test]
fn seek_lands_early_enough_for_the_reservoir_and_overlap() {
    // MPEG-1 at 128 kbit/s: 417 - 4 - 32 = 381 bytes of main data per frame, so two frames
    // hold the 511-byte reservoir; the frame before the target must decode in full.
    let bytes = frames(H128, 20);
    let mut d = open_bytes(seekable(&bytes)).unwrap();
    let pts_of = |i: i64| -529 + 1152 * i;
    // Target in frame 10: needs frame 9, whose reservoir comes from frames 7 and 8.
    assert_eq!(d.seek(pts_of(10) + 5).unwrap(), pts_of(7));
    let mut p = Packet::default();
    assert!(d.read_packet(&mut p).unwrap());
    assert_eq!((p.pts, &p.data), (pts_of(7), &frame(H128, 7)));
    // Backwards, repeated, and to the start.
    assert_eq!(d.seek(pts_of(3)).unwrap(), pts_of(0));
    assert_eq!(d.seek(pts_of(3)).unwrap(), pts_of(0));
    assert_eq!(d.seek(0).unwrap(), pts_of(0));
    assert_eq!(d.seek(i64::MIN).unwrap(), pts_of(0));
    assert!(d.read_packet(&mut p).unwrap());
    assert_eq!(p.data, frame(H128, 0));
    // At or past the end: nothing more, and the end's pts.
    let end = 20 * 1152 - 529;
    assert_eq!(d.seek(end).unwrap(), end);
    assert!(!d.read_packet(&mut p).unwrap());
    assert_eq!(d.seek(i64::MAX).unwrap(), end);
    assert!(!d.read_packet(&mut p).unwrap());
    // And back again from the end.
    assert_eq!(d.seek(pts_of(19)).unwrap(), pts_of(16));
    assert_eq!(read_all(&mut *d).unwrap().len(), 4);

    // MPEG-2: 192 - 4 - 17 = 171 bytes per frame, 255 needs two; two frames before the
    // target decode in full.
    let bytes = lsf_stream(20);
    let mut d = open_bytes(seekable(&bytes)).unwrap();
    let pts_of = |i: i64| -529 + 576 * i;
    assert_eq!(d.seek(pts_of(10)).unwrap(), pts_of(6));
}

#[test]
fn seeking_a_pipe_is_unsupported() {
    let mut d = open_bytes(pipe(&frames(H128, 3))).unwrap();
    assert!(matches!(d.seek(0), Err(Error::Unsupported { .. })));
}

#[test]
fn seek_after_an_info_frame_counts_from_the_first_audio_frame() {
    let mut bytes = lame_frame(10, 576, 1000);
    bytes.extend(frames(H128, 10));
    let mut d = open_bytes(seekable(&bytes)).unwrap();
    let first = -(576 + 529);
    assert_eq!(d.seek(first + 1152 * 5).unwrap(), first + 1152 * 2);
    let mut p = Packet::default();
    assert!(d.read_packet(&mut p).unwrap());
    assert_eq!(p.data, frame(H128, 2));
    let end = 10 * 1152 - 576 - 1000;
    assert_eq!(d.seek(end + 3).unwrap(), end);
}

#[test]
fn hostile_bytes_never_panic() {
    let mut seed = 0x1234_5678_u64;
    let mut rand = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let base = [frames(H128, 4), frames(H_LSF, 2)].concat();
    for _ in 0..300 {
        let mut b = base.clone();
        for _ in 0..(rand() % 16) {
            let i = (rand() as usize) % b.len();
            b[i] = rand() as u8;
        }
        b.truncate((rand() as usize) % (b.len() + 1));
        for src in [seekable(&b), pipe(&b)] {
            if let Ok(mut d) = open_bytes(src) {
                let _ = read_all(&mut *d);
                let _ = d.seek((rand() % 10_000) as i64 - 1000);
                let _ = read_all(&mut *d);
            }
        }
    }
}

/// One ID3v2 frame: id, size (syncsafe in v2.4), flags, body.
fn id3_frame(version: u8, id: &[u8], flags: u16, body: &[u8]) -> Vec<u8> {
    let n = body.len() as u32;
    let mut f = id.to_vec();
    match version {
        2 => f.extend(&n.to_be_bytes()[1..]),
        3 => f.extend(n.to_be_bytes()),
        _ => f.extend((0..4).rev().map(|i| ((n >> (7 * i)) & 0x7F) as u8)),
    }
    if version != 2 {
        f.extend(flags.to_be_bytes());
    }
    f.extend(body);
    f
}

fn id3_tag(version: u8, flags: u8, body: &[u8]) -> Vec<u8> {
    let n = body.len() as u32;
    let mut t = vec![b'I', b'D', b'3', version, 0, flags];
    t.extend((0..4).rev().map(|i| ((n >> (7 * i)) & 0x7F) as u8));
    t.extend(body);
    t
}

fn utf16le_bom(s: &str) -> Vec<u8> {
    let mut v = vec![0xFF, 0xFE];
    v.extend(s.encode_utf16().flat_map(u16::to_le_bytes));
    v
}

fn tags_of(tag: &[u8]) -> Vec<(String, String)> {
    let mut t = Tags::new();
    id3::parse_v2(tag, &mut t);
    t.iter().map(|x| (x.key.clone(), x.value.clone())).collect()
}

fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn id3v23_utf16_text_comment_and_genre_reference() {
    let body = [
        id3_frame(
            3,
            b"TIT2",
            0,
            &[&[1][..], &utf16le_bom("Ünïcode title")].concat(),
        ),
        id3_frame(3, b"TPE1", 0, b"\x00Artist\x00"),
        id3_frame(3, b"TCON", 0, b"\x00(17)"),
        id3_frame(
            3,
            b"COMM",
            0,
            &[
                &[1][..],
                b"eng",
                &utf16le_bom(""),
                &[0, 0],
                &utf16le_bom("A comment"),
            ]
            .concat(),
        ),
        id3_frame(3, b"COMM", 0, b"\x00engiTunNORM\x00 0000"),
        id3_frame(3, b"APIC", 0, &[0u8; 64]),
        id3_frame(3, b"TYER", 0, b"\x002001"),
        id3_frame(3, b"TRCK", 0, b"\x003/12"),
        vec![0; 20], // padding
    ]
    .concat();
    assert_eq!(
        tags_of(&id3_tag(3, 0, &body)),
        kv(&[
            ("TITLE", "Ünïcode title"),
            ("ARTIST", "Artist"),
            ("GENRE", "Rock"),
            ("COMMENT", "A comment"),
            ("DATE", "2001"),
            ("TRACKNUMBER", "3/12"),
        ])
    );
}

#[test]
fn id3v24_utf8_several_values_utf16be_and_long_syncsafe_frames() {
    let long = "x".repeat(300);
    let body = [
        id3_frame(4, b"TPE1", 0, b"\x03First\x00Second"),
        id3_frame(4, b"TALB", 0, &[&[3][..], long.as_bytes()].concat()),
        id3_frame(
            4,
            b"TIT2",
            0,
            &[
                &[2][..],
                &"Big end"
                    .encode_utf16()
                    .flat_map(u16::to_be_bytes)
                    .collect::<Vec<_>>(),
            ]
            .concat(),
        ),
        id3_frame(4, b"TCON", 0, b"\x0313\x00Synthwave"),
        id3_frame(4, b"TDRC", 0, b"\x032024-05-01"),
        id3_frame(4, b"TPE2", 0, b"\x03Band"),
        id3_frame(4, b"TPOS", 0, b"\x031/2"),
        id3_frame(4, b"TCOM", 0, b"\x03Composer"),
        id3_frame(4, b"TXXX", 0, b"\x03ignored\x00value"),
    ]
    .concat();
    assert_eq!(
        tags_of(&id3_tag(4, 0, &body)),
        kv(&[
            ("ARTIST", "First"),
            ("ARTIST", "Second"),
            ("ALBUM", &long),
            ("TITLE", "Big end"),
            ("GENRE", "Pop"),
            ("GENRE", "Synthwave"),
            ("DATE", "2024-05-01"),
            ("ALBUMARTIST", "Band"),
            ("DISCNUMBER", "1/2"),
            ("COMPOSER", "Composer"),
        ])
    );
}

#[test]
fn id3v24_frame_unsynchronisation_data_length_and_an_extended_header() {
    // "\xFF\x00" in the frame stands for "\xFF"; the four data-length bytes come first.
    let frame = id3_frame(
        4,
        b"TIT2",
        0x0003,
        &[&[0, 0, 0, 3][..], b"\x00a\xFF\x00b"].concat(),
    );
    let ext = [0, 0, 0, 6, 1, 0];
    let body = [&ext[..], &frame].concat();
    assert_eq!(
        tags_of(&id3_tag(4, 0x40, &body)),
        kv(&[("TITLE", "a\u{FF}b")])
    );
}

#[test]
fn id3v23_whole_tag_unsynchronisation_and_extended_header() {
    let frame = id3_frame(3, b"TIT2", 0, b"\x00\xFFtitle");
    let ext = [0, 0, 0, 6, 0, 0, 0, 0, 0, 0];
    let mut body = [&ext[..], &frame].concat();
    // Unsynchronise: an FF gets a 00 after it.
    let mut unsync = Vec::new();
    for b in body.drain(..) {
        unsync.push(b);
        if b == 0xFF {
            unsync.push(0);
        }
    }
    assert_eq!(
        tags_of(&id3_tag(3, 0xC0, &unsync)),
        kv(&[("TITLE", "\u{FF}title")])
    );
}

#[test]
fn id3v22_three_letter_frames() {
    let body = [
        id3_frame(2, b"TT2", 0, b"\x00Old title"),
        id3_frame(2, b"TP1", 0, b"\x00Old artist"),
        id3_frame(2, b"TCO", 0, b"\x00(13)"),
        id3_frame(2, b"TYE", 0, b"\x001988"),
        id3_frame(2, b"PIC", 0, &[1; 30]),
        id3_frame(2, b"COM", 0, b"\x00eng\x00old comment"),
    ]
    .concat();
    assert_eq!(
        tags_of(&id3_tag(2, 0, &body)),
        kv(&[
            ("TITLE", "Old title"),
            ("ARTIST", "Old artist"),
            ("GENRE", "Pop"),
            ("DATE", "1988"),
            ("COMMENT", "old comment"),
        ])
    );
    // v2.2 compression has no defined scheme: the whole tag is skipped.
    assert!(tags_of(&id3_tag(2, 0x40, &body)).is_empty());
}

#[test]
fn id3v1_fills_only_what_id3v2_did_not_give() {
    let tag = [
        id3_tag(3, 0, &id3_frame(3, b"TIT2", 0, b"\x00v2 title")),
        frames(H128, 2),
        id3v1("v1 title", 4, 13),
    ]
    .concat();
    let (_, d) = open_demuxer(seekable(&tag), &Limits::default()).unwrap();
    let t = d.tags();
    assert_eq!(t.get(tag_keys::TITLE), Some("v2 title"));
    assert_eq!(t.iter().filter(|x| x.key == "TITLE").count(), 1);
    assert_eq!(t.get(tag_keys::ARTIST), Some("Artist"));
    assert_eq!(t.get(tag_keys::GENRE), Some("Pop"));
    assert_eq!(t.get(tag_keys::TRACKNUMBER), Some("4"));
}

#[test]
fn genre_forms() {
    for (raw, want) in [
        ("(17)", "Rock"),
        ("(17)Custom rock", "Custom rock"),
        ("17", "Rock"),
        ("(RX)", "Remix"),
        ("(CR)", "Cover"),
        ("(999)", "(999)"),
        ("Jazz", "Jazz"),
        ("(", "("),
    ] {
        let body = id3_frame(4, b"TCON", 0, &[&[3][..], raw.as_bytes()].concat());
        assert_eq!(
            tags_of(&id3_tag(4, 0, &body)),
            kv(&[("GENRE", want)]),
            "{raw}"
        );
    }
}

#[test]
fn hostile_id3_tags_never_panic() {
    let good = id3_tag(
        4,
        0x40,
        &[
            &[0, 0, 0, 6, 1, 0][..],
            &id3_frame(4, b"TIT2", 0x0003, b"\x00\x00\x00\x02\x01\xFF\xFE\xFF"),
            &id3_frame(4, b"COMM", 0, b"\x01e"),
            &id3_frame(4, b"TCON", 0, b""),
        ]
        .concat(),
    );
    let mut seed = 99u64;
    for _ in 0..2000 {
        let mut t = good.clone();
        for _ in 0..4 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let i = (seed >> 33) as usize % t.len();
            t[i] = (seed >> 13) as u8;
        }
        let cut = (seed >> 40) as usize % (t.len() + 1);
        for v in [2, 3, 4] {
            t[3] = v;
            let _ = tags_of(&t[..cut]);
            let _ = tags_of(&t);
        }
    }
}
