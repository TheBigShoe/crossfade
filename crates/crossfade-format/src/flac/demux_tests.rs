use super::*;
use crossfade_core::Packet;
use crossfade_flac::bits::BitWriter;
use crossfade_flac::crc::crc16;
use crossfade_flac::frame::{self, ChannelAssignment, FrameHeader};
use crossfade_flac::streaminfo::StreamInfoBlock;
use crossfade_testkit::io::FaultySource;
use std::io::{Cursor, ErrorKind};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/flac/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn source(bytes: &[u8]) -> Box<dyn MediaSource> {
    Box::new(Cursor::new(bytes.to_vec()))
}

fn pipe(bytes: &[u8], max_read: usize) -> Box<dyn MediaSource> {
    Box::new(
        FaultySource::new(Cursor::new(bytes.to_vec()))
            .not_seekable()
            .max_read(max_read),
    )
}

fn open_bytes(bytes: &[u8]) -> Result<Box<dyn Demuxer>, Error> {
    open(source(bytes), &Limits::default())
}

/// Every packet, or the packets read before the error.
fn drain(d: &mut dyn Demuxer) -> (Vec<Packet>, Option<Error>) {
    let mut out = Vec::new();
    loop {
        let mut p = Packet::default();
        match d.read_packet(&mut p) {
            Ok(true) => out.push(p),
            Ok(false) => return (out, None),
            Err(e) => return (out, Some(e)),
        }
    }
}

fn all_packets(bytes: &[u8]) -> Vec<Packet> {
    let mut d = open_bytes(bytes).unwrap();
    let (packets, err) = drain(d.as_mut());
    assert_eq!(err, None);
    packets
}

fn malformed(e: Error) -> String {
    match e {
        Error::Malformed { what } => what,
        other => panic!("expected Malformed, got {other:?}"),
    }
}

fn err_of(r: Result<Box<dyn Demuxer>, Error>) -> Error {
    match r {
        Ok(_) => panic!("expected an error"),
        Err(e) => e,
    }
}

const BLOCK: u32 = 40;

fn info(total: u64) -> StreamInfoBlock {
    StreamInfoBlock {
        min_block_size: BLOCK as u16,
        max_block_size: BLOCK as u16,
        min_frame_size: 0,
        max_frame_size: 0,
        sample_rate: 8000,
        channels: 1,
        bits_per_sample: 8,
        total_samples: total,
        md5: [0; 16],
    }
}

fn block(kind: u8, last: bool, body: &[u8]) -> Vec<u8> {
    let len = body.len() as u32;
    let mut out = vec![kind | if last { 0x80 } else { 0 }];
    out.extend_from_slice(&len.to_be_bytes()[1..]);
    out.extend_from_slice(body);
    out
}

fn header(number: u64) -> FrameHeader {
    FrameHeader {
        variable_block_size: false,
        block_size: BLOCK,
        sample_rate: None,
        channels: ChannelAssignment::Independent(1),
        bits_per_sample: Some(8),
        number,
        header_len: 0,
    }
}

/// A mono 8-bit verbatim frame whose payload is `payload` (BLOCK bytes).
fn verbatim_frame(number: u64, payload: &[u8]) -> Vec<u8> {
    assert_eq!(payload.len(), BLOCK as usize);
    let mut f = Vec::new();
    frame::write_header(&header(number), &mut f);
    let mut w = BitWriter::new();
    w.write_bits(0b0000_0010, 8);
    for &b in payload {
        w.write_bits(u64::from(b), 8);
    }
    f.extend(w.into_bytes());
    let crc = crc16(&f);
    f.extend_from_slice(&crc.to_be_bytes());
    f
}

fn plain_payload(k: u64) -> Vec<u8> {
    (0..BLOCK)
        .map(|i| ((u64::from(i) * 7 + k) & 0x7F) as u8)
        .collect()
}

fn frames(n: u64) -> Vec<Vec<u8>> {
    (0..n)
        .map(|k| verbatim_frame(k, &plain_payload(k)))
        .collect()
}

/// `fLaC`, STREAMINFO, `extra` blocks (the last one flagged), then `frames`.
fn stream(total: u64, extra: &[(u8, Vec<u8>)], frames: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"fLaC".to_vec();
    out.extend(block(0, extra.is_empty(), &info(total).write()));
    for (i, (kind, body)) in extra.iter().enumerate() {
        out.extend(block(*kind, i + 1 == extra.len(), body));
    }
    for f in frames {
        out.extend_from_slice(f);
    }
    out
}

fn comment(fields: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(b"abc");
    out.extend_from_slice(&(fields.len() as u32).to_le_bytes());
    for f in fields {
        out.extend_from_slice(&(f.len() as u32).to_le_bytes());
        out.extend_from_slice(f.as_bytes());
    }
    out
}

// ---- metadata ----

#[test]
fn the_stream_comes_from_streaminfo() {
    let bytes = fixture("stereo-16bit.flac");
    let d = open_bytes(&bytes).unwrap();
    let s = &d.streams()[0];
    assert_eq!(d.streams().len(), 1);
    assert_eq!(s.codec, crossfade_core::CodecId::Flac);
    assert_eq!(s.audio.sample_rate(), 44100);
    assert_eq!(s.audio.layout().channels(), 2);
    assert_eq!(s.frames, Some(5000));
    assert_eq!(s.extradata, bytes[8..42]);
}

#[test]
fn tags_come_from_the_vorbis_comment_block() {
    let bytes = fixture("tagged.flac");
    let d = open_bytes(&bytes).unwrap();
    assert_eq!(d.tags().get("TITLE"), Some("Fixture"));
    assert_eq!(d.tags().get("ARTIST"), Some("Crossfade"));
    // Seek table, picture and padding were skipped on the way to the audio.
    let (packets, err) = drain(open_bytes(&bytes).unwrap().as_mut());
    assert_eq!(err, None);
    assert!(!packets.is_empty());
}

#[test]
fn a_channel_mask_comment_sets_the_layout() {
    let c = comment(&["WAVEFORMATEXTENSIBLE_CHANNEL_MASK=0x0001"]);
    let d = open_bytes(&stream(0, &[(4, c)], &[])).unwrap();
    assert_eq!(
        d.streams()[0].audio.layout().positions(),
        Some(&[crossfade_core::Channel::FrontLeft][..])
    );
    // The tag stays among the tags; the muxer decides what to write.
    assert!(d.tags().get("WAVEFORMATEXTENSIBLE_CHANNEL_MASK").is_some());
    let bad = comment(&["WAVEFORMATEXTENSIBLE_CHANNEL_MASK=0x3"]);
    let d = open_bytes(&stream(0, &[(4, bad)], &[])).unwrap();
    assert_eq!(
        d.streams()[0].audio.layout(),
        &crossfade_core::ChannelLayout::mono()
    );
}

#[test]
fn a_file_without_comments_has_no_tags() {
    assert!(open_bytes(&stream(0, &[], &[])).unwrap().tags().is_empty());
}

#[test]
fn streaminfo_must_come_first() {
    let mut bytes = b"fLaC".to_vec();
    bytes.extend(block(4, false, &comment(&[])));
    bytes.extend(block(0, true, &info(0).write()));
    assert!(malformed(err_of(open_bytes(&bytes))).contains("STREAMINFO"));
}

#[test]
fn a_streaminfo_of_the_wrong_length_is_malformed() {
    let mut bytes = b"fLaC".to_vec();
    bytes.extend(block(0, true, &info(0).write()[..33]));
    assert!(matches!(
        err_of(open_bytes(&bytes)),
        Error::Malformed { .. }
    ));
}

#[test]
fn block_type_127_is_malformed() {
    let bytes = stream(0, &[(127, vec![0; 4])], &[]);
    assert!(malformed(err_of(open_bytes(&bytes))).contains("127"));
}

#[test]
fn unknown_and_ignorable_block_types_are_skipped() {
    let extra: Vec<(u8, Vec<u8>)> = [1u8, 2, 3, 5, 6, 7, 42, 126]
        .iter()
        .map(|&k| (k, vec![0xFF; 20]))
        .collect();
    let bytes = stream(80, &extra, &frames(2));
    assert_eq!(all_packets(&bytes).len(), 2);
}

#[test]
fn an_id3v2_tag_before_the_marker_is_skipped_by_its_size() {
    let mut tag = b"ID3\x04\x00\x00\x00\x00\x00\x07".to_vec();
    tag.extend_from_slice(b"fLaC\xFF\xF8\x00"); // looks like what follows, but is the tag body
    let mut bytes = tag;
    bytes.extend(stream(80, &[], &frames(2)));
    assert_eq!(all_packets(&bytes).len(), 2);
    // A footer (flag 0x10) adds ten more bytes.
    let mut with_footer = b"ID3\x04\x00\x10\x00\x00\x00\x02".to_vec();
    with_footer.extend_from_slice(&[0; 2 + 10]);
    with_footer.extend(stream(80, &[], &frames(2)));
    assert_eq!(all_packets(&with_footer).len(), 2);
    // The real fixture.
    let d = open_bytes(&fixture("id3v2-prefixed.flac")).unwrap();
    assert_eq!(d.streams()[0].frames, Some(5000));
}

#[test]
fn a_marker_that_is_not_there_is_unrecognised() {
    assert_eq!(
        err_of(open_bytes(b"RIFF....WAVE")),
        Error::UnrecognisedFormat
    );
    assert_eq!(err_of(open_bytes(b"fL")), Error::UnrecognisedFormat);
    assert_eq!(err_of(open_bytes(b"")), Error::UnrecognisedFormat);
}

#[test]
fn a_truncated_block_is_an_unexpected_eof() {
    let mut bytes = stream(0, &[(1, vec![0; 100])], &[]);
    bytes.truncate(bytes.len() - 1);
    assert!(matches!(
        err_of(open_bytes(&bytes)),
        Error::UnexpectedEof { .. }
    ));
}

#[test]
fn oversized_declared_lengths_on_tiny_inputs_allocate_nothing() {
    let tiny = Limits {
        max_alloc_bytes: 64,
        max_nesting_depth: 4,
        max_packet_bytes: 64,
        ..Limits::default()
    };
    for kind in [1u8, 3, 4, 5, 6, 99] {
        let mut bytes = b"fLaC".to_vec();
        bytes.extend(block(0, false, &info(0).write()));
        bytes.push(kind | 0x80);
        bytes.extend_from_slice(&[0xFF, 0xFF, 0xFF]);
        bytes.extend_from_slice(b"short");
        let e = err_of(open(source(&bytes), &tiny));
        assert!(matches!(e, Error::UnexpectedEof { .. }), "{kind}: {e:?}");
    }
}

#[test]
fn a_comment_block_over_the_allocation_limit_is_skipped_not_loaded() {
    let tiny = Limits {
        max_alloc_bytes: 64,
        ..Limits::default()
    };
    let big = comment(&["TITLE=a title that is much longer than sixty-four bytes in total, see"]);
    assert!(big.len() > 64);
    let bytes = stream(80, &[(4, big)], &frames(2));
    let mut d = open(source(&bytes), &tiny).unwrap();
    assert!(d.tags().is_empty());
    assert_eq!(drain(d.as_mut()).0.len(), 2);
}

#[test]
fn a_comment_block_that_fails_to_parse_is_ignored() {
    // Claims ten fields, holds one.
    let mut body = comment(&["A=1"]);
    body[7] = 10;
    let bytes = stream(80, &[(4, body)], &frames(2));
    let mut d = open_bytes(&bytes).unwrap();
    assert!(d.tags().is_empty());
    assert_eq!(drain(d.as_mut()).0.len(), 2);
}

#[test]
fn only_the_first_comment_block_counts() {
    let bytes = stream(
        80,
        &[(4, comment(&["A=1"])), (4, comment(&["A=2", "B=3"]))],
        &frames(2),
    );
    let d = open_bytes(&bytes).unwrap();
    assert_eq!(d.tags().get("A"), Some("1"));
    assert_eq!(d.tags().get("B"), None);
}

// ---- packets ----

#[test]
fn packets_are_whole_frames_with_their_first_sample_as_pts() {
    let bytes = stream(200, &[], &frames(5));
    let packets = all_packets(&bytes);
    assert_eq!(packets.len(), 5);
    for (k, p) in packets.iter().enumerate() {
        assert_eq!(p.data, frames(5)[k]);
        assert_eq!(p.pts, k as i64 * i64::from(BLOCK));
        assert_eq!(p.duration, u64::from(BLOCK));
        assert_eq!(p.stream, 0);
    }
}

#[test]
fn every_fixture_splits_into_crc_valid_frames_that_cover_the_stream() {
    for name in [
        "mono-8bit.flac",
        "stereo-16bit.flac",
        "stereo-32bit.flac",
        "surround-5.1.flac",
        "block-16.flac",
        "block-65535.flac",
        "verbatim-noise.flac",
        "constant-silence.flac",
        "tagged.flac",
        "unknown-total.flac",
    ] {
        let bytes = fixture(name);
        let d = open_bytes(&bytes).unwrap();
        let total = d.streams()[0].frames;
        let (packets, err) = drain(open_bytes(&bytes).unwrap().as_mut());
        assert_eq!(err, None, "{name}");
        let mut next = 0u64;
        for p in &packets {
            assert_eq!(crc16(&p.data), 0, "{name}: frame at {next}");
            assert_eq!(p.pts as u64, next, "{name}");
            next += p.duration;
        }
        if let Some(total) = total {
            assert_eq!(next, total, "{name}");
        }
        assert!(next > 0, "{name}");
    }
}

#[test]
fn a_false_sync_inside_frame_data_does_not_split_the_frame() {
    // A payload that holds a well-formed frame header: its CRC-8 passes, but the CRC-16 of
    // the bytes before it does not, so it is not a frame boundary.
    let mut payload = plain_payload(1);
    let mut fake = Vec::new();
    frame::write_header(&header(2), &mut fake);
    payload[10..10 + fake.len()].copy_from_slice(&fake);
    let frames = [
        verbatim_frame(0, &plain_payload(0)),
        verbatim_frame(1, &payload),
        verbatim_frame(2, &plain_payload(2)),
    ];
    let packets = all_packets(&stream(120, &[], &frames));
    assert_eq!(packets.len(), 3);
    assert_eq!(packets[1].data, frames[1]);
}

#[test]
fn a_pipe_that_returns_one_byte_at_a_time_gives_the_same_packets() {
    let bytes = fixture("tagged.flac");
    let want = all_packets(&bytes);
    let mut d = open(pipe(&bytes, 1), &Limits::default()).unwrap();
    let (got, err) = drain(d.as_mut());
    assert_eq!(err, None);
    assert_eq!(got, want);
    assert_eq!(d.tags().get("TITLE"), Some("Fixture"));
}

#[test]
fn trailing_bytes_after_the_last_frame_are_ignored() {
    let want = all_packets(&stream(120, &[], &frames(3)));
    for tail in [
        // An ID3v1 tag.
        [b"TAG".as_slice(), &[b'x'; 125]].concat(),
        // A false header start that never completes.
        vec![0xFF, 0xF8, 0xC9],
    ] {
        let mut bytes = stream(120, &[], &frames(3));
        bytes.extend(&tail);
        assert_eq!(all_packets(&bytes), want, "{} trailing bytes", tail.len());
    }
}

#[test]
fn leading_junk_before_the_first_frame_is_skipped() {
    let mut bytes = stream(120, &[], &[]);
    bytes.extend_from_slice(&[0x55; 300]);
    bytes.extend_from_slice(&[0xFF, 0xF8, 0x00]);
    for f in frames(3) {
        bytes.extend(f);
    }
    assert_eq!(
        all_packets(&bytes),
        all_packets(&stream(120, &[], &frames(3)))
    );
}

#[test]
fn too_much_leading_junk_is_malformed() {
    let mut bytes = stream(120, &[], &[]);
    bytes.extend(vec![0x55; 70_000]);
    for f in frames(3) {
        bytes.extend(f);
    }
    let mut d = open_bytes(&bytes).unwrap();
    let mut p = Packet::default();
    assert!(malformed(d.read_packet(&mut p).unwrap_err()).contains("frame"));
}

#[test]
fn junk_with_no_frame_at_all_is_malformed() {
    let mut bytes = stream(0, &[], &[]);
    bytes.extend(vec![0x55; 1000]);
    let mut d = open_bytes(&bytes).unwrap();
    let mut p = Packet::default();
    assert!(matches!(
        d.read_packet(&mut p),
        Err(Error::Malformed { .. })
    ));
}

#[test]
fn a_stream_with_no_frames_ends_at_once() {
    let mut d = open_bytes(&stream(0, &[], &[])).unwrap();
    let mut p = Packet::default();
    assert_eq!(d.read_packet(&mut p), Ok(false));
}

#[test]
fn a_frame_cut_off_by_the_end_of_input_is_an_unexpected_eof() {
    let mut bytes = stream(120, &[], &frames(3));
    bytes.truncate(bytes.len() - 5);
    let mut d = open_bytes(&bytes).unwrap();
    let (packets, err) = drain(d.as_mut());
    assert_eq!(packets.len(), 2);
    assert!(matches!(err, Some(Error::UnexpectedEof { .. })), "{err:?}");
}

#[test]
fn a_frame_longer_than_the_packet_limit_is_malformed() {
    let bytes = stream(120, &[], &frames(3));
    let limits = Limits {
        max_packet_bytes: 20,
        ..Limits::default()
    };
    let mut d = open(source(&bytes), &limits).unwrap();
    let mut p = Packet::default();
    assert!(malformed(d.read_packet(&mut p).unwrap_err()).contains("longer"));
}

#[test]
fn a_frame_longer_than_streaminfos_maximum_is_malformed() {
    let mut si = info(120);
    si.max_frame_size = 30;
    let mut bytes = b"fLaC".to_vec();
    bytes.extend(block(0, true, &si.write()));
    for f in frames(3) {
        bytes.extend(f);
    }
    let mut d = open_bytes(&bytes).unwrap();
    let mut p = Packet::default();
    assert!(malformed(d.read_packet(&mut p).unwrap_err()).contains("longer"));
}

#[test]
fn a_frame_in_the_other_blocking_strategy_is_not_a_boundary() {
    // The second frame's payload embeds a variable-block-size header; the stream is fixed.
    let mut fake = Vec::new();
    frame::write_header(
        &FrameHeader {
            variable_block_size: true,
            ..header(2000)
        },
        &mut fake,
    );
    let mut payload = plain_payload(1);
    payload[4..4 + fake.len()].copy_from_slice(&fake);
    let mut f1 = verbatim_frame(1, &payload);
    // Repair nothing: the CRC-16 was computed over the embedded header already.
    assert_eq!(crc16(&f1), 0);
    let frames = [
        verbatim_frame(0, &plain_payload(0)),
        std::mem::take(&mut f1),
    ];
    assert_eq!(all_packets(&stream(80, &[], &frames)).len(), 2);
}

#[test]
fn truncating_a_small_stream_at_every_offset_never_panics() {
    let bytes = stream(120, &[(4, comment(&["A=1"])), (1, vec![0; 9])], &frames(3));
    let full = all_packets(&bytes);
    for n in 0..bytes.len() {
        let Ok(mut d) = open_bytes(&bytes[..n]) else {
            continue;
        };
        // Whole frames come out intact and in order; what follows is an error or, when the
        // cut fell between frames, a clean end.
        let (packets, _) = drain(d.as_mut());
        assert!(packets.len() <= full.len(), "{n}");
        assert_eq!(packets[..], full[..packets.len()], "{n}");
    }
}

#[test]
fn truncating_a_real_file_at_every_offset_never_panics() {
    let bytes = fixture("mono-8bit.flac");
    let full = all_packets(&bytes);
    for n in (0..bytes.len()).step_by(7) {
        if let Ok(mut d) = open_bytes(&bytes[..n]) {
            let (packets, _) = drain(d.as_mut());
            assert_eq!(packets[..], full[..packets.len()], "{n}");
        }
    }
}

#[test]
fn a_source_that_fails_mid_frame_reports_the_io_error() {
    let bytes = stream(120, &[], &frames(3));
    let at = bytes.len() as u64 - 30;
    let src = FaultySource::new(Cursor::new(bytes)).fail_at(at, ErrorKind::BrokenPipe);
    let mut d = open(Box::new(src), &Limits::default()).unwrap();
    let (packets, err) = drain(d.as_mut());
    assert_eq!(packets.len(), 2);
    assert!(matches!(err, Some(Error::Io { .. })), "{err:?}");
}

#[test]
fn variable_block_size_streams_use_the_coded_sample_number_as_pts() {
    let mut out = Vec::new();
    for (start, size) in [(0u64, 40u32), (40, 40), (80, 17)] {
        let h = FrameHeader {
            variable_block_size: true,
            block_size: size,
            number: start,
            ..header(0)
        };
        let mut f = Vec::new();
        frame::write_header(&h, &mut f);
        let mut w = BitWriter::new();
        w.write_bits(0b0000_0010, 8);
        for i in 0..size {
            w.write_bits(u64::from(i * 3 % 100), 8);
        }
        f.extend(w.into_bytes());
        let crc = crc16(&f);
        f.extend_from_slice(&crc.to_be_bytes());
        out.push(f);
    }
    let packets = all_packets(&stream(97, &[], &out));
    let got: Vec<(i64, u64)> = packets.iter().map(|p| (p.pts, p.duration)).collect();
    assert_eq!(got, [(0, 40), (40, 40), (80, 17)]);
}

/// A payload that makes the frame's CRC-16 end in a zero byte, so the CRC-16 over the frame
/// minus its last byte is already zero.
fn payload_with_crc_low_byte_zero() -> (Vec<u8>, Vec<u8>) {
    for k in 0..4096u64 {
        let p = plain_payload(k);
        let f = verbatim_frame(2, &p);
        if f[f.len() - 1] == 0 {
            return (p, f);
        }
    }
    panic!("no payload found");
}

#[test]
fn a_last_frame_whose_crc_ends_in_a_zero_byte_is_kept_whole() {
    let (_, last) = payload_with_crc_low_byte_zero();
    let mut f = frames(2);
    f.push(last.clone());
    let packets = all_packets(&stream(120, &[], &f));
    assert_eq!(packets.len(), 3);
    assert_eq!(packets[2].data, last);
}

#[test]
fn zero_padding_after_the_last_frame_is_left_to_the_decoder() {
    // The CRC-16 stays zero over zero bytes, so the padding cannot be told from a frame whose
    // CRC ends in zeros; the reader keeps it with the frame and the decoder skips it.
    let mut bytes = stream(120, &[], &frames(3));
    bytes.extend([0u8; 9]);
    let packets = all_packets(&bytes);
    assert_eq!(packets.len(), 3);
    assert!(packets[2].data.starts_with(&frames(3)[2]));
    assert!(
        packets[2].data[frames(3)[2].len()..]
            .iter()
            .all(|&b| b == 0)
    );
}

mod find_ff {
    use super::super::find_ff;
    use proptest::prelude::*;

    proptest! {
        // The word-at-a-time scan must find what a byte loop finds, on data rich in 0xFF and 0xFE
        // (the bytes the has-zero test is most likely to get wrong).
        #[test]
        fn equals_a_byte_loop(
            data in prop::collection::vec(prop_oneof![Just(0xFFu8), Just(0xFE), Just(0), any::<u8>()], 0..64),
        ) {
            prop_assert_eq!(find_ff(&data), data.iter().position(|&b| b == 0xFF));
        }
    }
}

/// A mono 8-bit verbatim frame for `h` whose audio holds a false frame header after a prefix
/// whose CRC-16 is zero: the bytes of the frame up to the prefix's end are followed by their own
/// CRC, then `fake`. Returns the frame and the offset of the false header in it.
fn frame_with_false_header(h: FrameHeader, fake: &FrameHeader) -> (Vec<u8>, usize) {
    assert!(h.block_size >= 40);
    let mut f = Vec::new();
    frame::write_header(&h, &mut f);
    f.push(0b0000_0010);
    f.extend_from_slice(&[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x08]);
    let crc = crc16(&f);
    f.extend_from_slice(&crc.to_be_bytes());
    assert_eq!(crc16(&f), 0, "the prefix is CRC-valid");
    let at = f.len();
    frame::write_header(fake, &mut f);
    let body = 1 + h.block_size as usize;
    while f.len() < body + frame_header_len(&h) {
        f.push(0x35);
    }
    let crc = crc16(&f);
    f.extend_from_slice(&crc.to_be_bytes());
    (f, at)
}

fn frame_header_len(h: &FrameHeader) -> usize {
    let mut v = Vec::new();
    frame::write_header(h, &mut v);
    v.len()
}

fn variable_header(start: u64, size: u32) -> FrameHeader {
    FrameHeader {
        variable_block_size: true,
        block_size: size,
        number: start,
        ..header(0)
    }
}

fn plain_frame(h: FrameHeader) -> Vec<u8> {
    let mut f = Vec::new();
    frame::write_header(&h, &mut f);
    f.push(0b0000_0010);
    f.extend((0..h.block_size).map(|i| ((i * 3 + 1) % 100) as u8));
    let crc = crc16(&f);
    f.extend_from_slice(&crc.to_be_bytes());
    f
}

#[test]
fn a_false_header_after_a_zero_crc_prefix_with_the_wrong_number_is_not_a_boundary() {
    let (f1, _) = frame_with_false_header(header(1), &header(9));
    let frames = [
        verbatim_frame(0, &plain_payload(0)),
        f1,
        verbatim_frame(2, &plain_payload(2)),
    ];
    let packets = all_packets(&stream(120, &[], &frames));
    assert_eq!(packets.len(), 3);
    for (p, f) in packets.iter().zip(&frames) {
        assert_eq!(&p.data, f);
    }
}

#[test]
fn the_same_false_header_with_the_right_number_cannot_be_told_from_a_boundary() {
    // Without decoding, a CRC-valid prefix followed by a header that carries the number the next
    // frame should have is a frame boundary. The decoder then refuses the cut-off frame.
    let (f1, at) = frame_with_false_header(header(1), &header(2));
    let frames = [
        verbatim_frame(0, &plain_payload(0)),
        f1.clone(),
        verbatim_frame(2, &plain_payload(2)),
    ];
    let bytes = stream(120, &[], &frames);
    let packets = all_packets(&bytes);
    assert_eq!(packets[1].data, f1[..at]);
    let mut dec = crossfade_flac::Decoder::new(&info(120)).unwrap();
    assert!(dec.decode_frame(&packets[0].data).is_ok());
    assert!(dec.decode_frame(&packets[1].data).is_err());
}

#[test]
fn a_variable_block_size_stream_follows_the_start_plus_the_block_size() {
    let (f1, _) = frame_with_false_header(variable_header(40, 40), &variable_header(7, 40));
    let frames = [
        plain_frame(variable_header(0, 40)),
        f1,
        plain_frame(variable_header(80, 40)),
    ];
    let packets = all_packets(&stream(120, &[], &frames));
    assert_eq!(packets.len(), 3);
    for (p, f) in packets.iter().zip(&frames) {
        assert_eq!(&p.data, f);
    }
}

#[test]
fn a_real_number_jump_is_still_a_boundary() {
    // Numbers 0, 1, 5, 6: the jump at the third frame is genuine, nothing later fits better,
    // so the first CRC-valid boundary stands, as it always did.
    let numbers = [0u64, 1, 5, 6];
    let frames: Vec<Vec<u8>> = numbers
        .iter()
        .map(|&n| verbatim_frame(n, &plain_payload(n)))
        .collect();
    let packets = all_packets(&stream(160, &[], &frames));
    assert_eq!(packets.len(), 4);
    for (p, f) in packets.iter().zip(&frames) {
        assert_eq!(&p.data, f);
    }
    // And with a frame after the jump that is not consecutive either.
    let numbers = [0u64, 7, 3, 12, 13];
    let frames: Vec<Vec<u8>> = numbers
        .iter()
        .map(|&n| verbatim_frame(n, &plain_payload(n)))
        .collect();
    assert_eq!(all_packets(&stream(200, &[], &frames)).len(), 5);
}

#[test]
fn a_false_header_in_the_last_frame_with_other_stream_parameters_is_not_a_boundary() {
    // At the end of input the reader cannot look for a later boundary; a candidate whose header
    // disagrees with the frame's on channels, depth or rate is not trusted then.
    let mut fake = header(9);
    fake.channels = ChannelAssignment::Independent(2);
    let (f1, _) = frame_with_false_header(header(1), &fake);
    let frames = [verbatim_frame(0, &plain_payload(0)), f1];
    let packets = all_packets(&stream(80, &[], &frames));
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[1].data, frames[1]);
}

#[test]
fn frames_with_wrong_numbers_are_scanned_in_linear_time() {
    // Thousands of tiny frames, none carrying the number its predecessor announces: every
    // frame has a boundary but none "matches", so each read could rescan everything up to the
    // frame-length cap. The running CRC and the remembered candidates keep it linear.
    let h = |n: u64| FrameHeader {
        block_size: 4096,
        channels: ChannelAssignment::Independent(2),
        bits_per_sample: Some(8),
        number: n,
        ..header(0)
    };
    let tiny = |n: u64| {
        let mut f = Vec::new();
        frame::write_header(&h(n), &mut f);
        // Two constant subframes.
        f.extend_from_slice(&[0x00, 0x05, 0x00, 0x06]);
        let crc = crc16(&f);
        f.extend_from_slice(&crc.to_be_bytes());
        f
    };
    let count = 4000u64;
    let frames: Vec<Vec<u8>> = (0..count).map(|k| tiny(k * 3)).collect();
    let mut bytes = stream(count * 4096, &[], &frames);
    // `info` says mono; the frames say stereo, which the demuxer does not compare.
    let total = bytes.len() as u64;
    CRC_STEPS.with(|c| c.set(0));
    let packets = all_packets(&bytes);
    let steps = CRC_STEPS.with(|c| c.get());
    assert_eq!(packets.len(), count as usize);
    assert!(
        steps <= 2 * total,
        "{steps} CRC byte-steps for {total} bytes"
    );
    bytes.clear();
}
