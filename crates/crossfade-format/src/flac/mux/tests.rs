use super::*;
use crate::vorbis_comment;
use crossfade_core::{AudioSpec, ChannelLayout, CodecId, Limits, Packet, SampleFormat, TimeBase};
use crossfade_flac::streaminfo::StreamInfoBlock;
use std::io::{Cursor, SeekFrom};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct State {
    bytes: Cursor<Vec<u8>>,
    /// Where the cursor stood when `finish` ran.
    at_finish: Option<u64>,
    finished: u32,
}

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<State>>);

impl Shared {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().bytes.get_ref().clone()
    }
}

impl MediaSink for Shared {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        std::io::Write::write_all(&mut self.0.lock().unwrap().bytes, buf)
    }
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        std::io::Seek::seek(&mut self.0.lock().unwrap().bytes, pos)
    }
    fn finish(&mut self) -> std::io::Result<()> {
        let mut s = self.0.lock().unwrap();
        s.at_finish = Some(s.bytes.position());
        s.finished += 1;
        Ok(())
    }
}

/// A pipe: writes work, seeking does not.
#[derive(Clone, Default)]
struct Pipe(Shared);

impl MediaSink for Pipe {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.0.write_all(buf)
    }
    fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
        Err(std::io::ErrorKind::NotSeekable.into())
    }
    fn finish(&mut self) -> std::io::Result<()> {
        self.0.finish()
    }
}

fn block(total: u64, md5: u8) -> StreamInfoBlock {
    StreamInfoBlock {
        min_block_size: 4096,
        max_block_size: 4096,
        min_frame_size: if total == 0 { 0 } else { 1443 },
        max_frame_size: if total == 0 { 0 } else { 3446 },
        sample_rate: 44100,
        channels: 2,
        bits_per_sample: 16,
        total_samples: total,
        md5: [md5; 16],
    }
}

fn stream(extradata: &[u8]) -> StreamInfo {
    let audio = AudioSpec::new(44100, SampleFormat::S16, ChannelLayout::stereo()).unwrap();
    StreamInfo {
        extradata: extradata.to_vec(),
        ..StreamInfo::new(0, CodecId::Flac, audio, TimeBase::new(1, 44100).unwrap())
    }
}

fn vendor() -> String {
    format!("Crossfade {}", env!("CARGO_PKG_VERSION"))
}

fn tags() -> Tags {
    let mut t = Tags::new();
    t.push("TITLE", "Tone");
    t.push("ARTIST", "A");
    t.push("ARTIST", "B");
    t
}

fn packet(data: &[u8]) -> Packet {
    Packet {
        data: data.to_vec(),
        ..Packet::default()
    }
}

/// `fLaC`, STREAMINFO, VORBIS_COMMENT: the header every file starts with.
fn expected_header(info: &[u8], tags: &Tags) -> Vec<u8> {
    let mut comment = Vec::new();
    vorbis_comment::write(&vendor(), tags, &mut comment);
    let mut h = b"fLaC".to_vec();
    h.extend([0x00, 0, 0, 34]);
    h.extend(info);
    h.push(0x84);
    h.extend(&(comment.len() as u32).to_be_bytes()[1..]);
    h.extend(comment);
    h
}

#[test]
fn the_header_is_marker_streaminfo_then_a_last_vorbis_comment_block() {
    let info = block(0, 0).write();
    let sink = Shared::default();
    let mut m = create(Box::new(sink.clone()), &stream(&info), &tags()).unwrap();
    m.finish().unwrap();
    assert_eq!(sink.bytes(), expected_header(&info, &tags()));
}

#[test]
fn a_comment_block_is_written_even_without_tags() {
    let info = block(0, 0).write();
    let sink = Shared::default();
    let mut m = create(Box::new(sink.clone()), &stream(&info), &Tags::new()).unwrap();
    m.finish().unwrap();
    let bytes = sink.bytes();
    assert_eq!(bytes, expected_header(&info, &Tags::new()));
    // Flag byte of STREAMINFO is clear, of the last block set.
    assert_eq!(bytes[4] & 0x80, 0);
    assert_eq!(bytes[42] & 0x80, 0x80);
    assert_eq!(bytes[42] & 0x7F, 4);
}

#[test]
fn tags_round_trip_through_the_vorbis_comment_parser() {
    let info = block(0, 0).write();
    let sink = Shared::default();
    let mut m = create(Box::new(sink.clone()), &stream(&info), &tags()).unwrap();
    m.finish().unwrap();
    let bytes = sink.bytes();
    let len = usize::from(bytes[43]) << 16 | usize::from(bytes[44]) << 8 | usize::from(bytes[45]);
    assert_eq!(bytes.len(), 46 + len);
    let parsed = vorbis_comment::parse(&bytes[46..], &Limits::default()).unwrap();
    assert_eq!(parsed.vendor, vendor());
    assert_eq!(parsed.tags, tags());
}

#[test]
fn packets_follow_the_metadata_verbatim() {
    let info = block(0, 0).write();
    let sink = Shared::default();
    let mut m = create(Box::new(sink.clone()), &stream(&info), &tags()).unwrap();
    m.write_packet(&packet(&[1, 2, 3])).unwrap();
    m.write_packet(&packet(&[4, 5])).unwrap();
    m.finish().unwrap();
    let mut want = expected_header(&info, &tags());
    want.extend([1, 2, 3, 4, 5]);
    assert_eq!(sink.bytes(), want);
}

#[test]
fn the_final_streaminfo_replaces_the_provisional_one_and_nothing_else_moves() {
    let provisional = block(0, 0).write();
    let last = block(123_456, 0xAB).write();
    let sink = Shared::default();
    let mut m = create(Box::new(sink.clone()), &stream(&provisional), &tags()).unwrap();
    m.write_packet(&packet(&[9; 100])).unwrap();
    m.set_extradata(&last).unwrap();
    m.finish().unwrap();
    let mut want = expected_header(&last, &tags());
    want.extend([9; 100]);
    let bytes = sink.bytes();
    assert_eq!(bytes, want);
    let s = sink.0.lock().unwrap();
    assert_eq!(s.at_finish, Some(bytes.len() as u64));
    assert_eq!(s.finished, 1);
}

#[test]
fn a_sink_that_cannot_seek_keeps_the_provisional_header_and_still_succeeds() {
    let provisional = block(0, 0).write();
    let pipe = Pipe::default();
    let mut m = create(Box::new(pipe.clone()), &stream(&provisional), &tags()).unwrap();
    m.write_packet(&packet(&[7; 10])).unwrap();
    m.set_extradata(&block(500, 1).write()).unwrap();
    m.finish().unwrap();
    let mut want = expected_header(&provisional, &tags());
    want.extend([7; 10]);
    assert_eq!(pipe.0.bytes(), want);
    assert_eq!(pipe.0.0.lock().unwrap().finished, 1);
}

#[test]
fn stream_copy_writes_the_source_streaminfo_untouched() {
    let source = block(99_999, 0x5A).write();
    let sink = Shared::default();
    let mut m = create(Box::new(sink.clone()), &stream(&source), &Tags::new()).unwrap();
    m.write_packet(&packet(&[1])).unwrap();
    m.finish().unwrap();
    let bytes = sink.bytes();
    assert_eq!(&bytes[8..42], &source[..]);
    assert_eq!(*bytes.last().unwrap(), 1);
}

#[test]
fn without_extradata_the_header_comes_from_the_audio_description() {
    let mut s = stream(&[]);
    s.frames = Some(1000);
    let sink = Shared::default();
    let mut m = create(Box::new(sink.clone()), &s, &Tags::new()).unwrap();
    m.finish().unwrap();
    let body = StreamInfoBlock {
        min_block_size: 0,
        max_block_size: 0,
        min_frame_size: 0,
        max_frame_size: 0,
        sample_rate: 44100,
        channels: 2,
        bits_per_sample: 16,
        total_samples: 1000,
        md5: [0; 16],
    }
    .write();
    assert_eq!(&sink.bytes()[8..42], &body[..]);
}

#[test]
fn the_synthesised_depth_follows_the_format_and_the_raw_bit_count() {
    let depth = |format, raw| {
        let audio = AudioSpec::new(8000, format, ChannelLayout::mono()).unwrap();
        let mut s = StreamInfo::new(0, CodecId::Flac, audio, TimeBase::new(1, 8000).unwrap());
        s.bits_per_raw_sample = raw;
        let sink = Shared::default();
        create(Box::new(sink.clone()), &s, &Tags::new())
            .unwrap()
            .finish()
            .unwrap();
        let bytes = sink.bytes();
        let body = &bytes[8..42];
        // 20 bit rate, 3 bit channels - 1, 5 bit depth - 1 sit at bits 80..108.
        (body[12] & 1) << 4 | body[13] >> 4
    };
    assert_eq!(depth(SampleFormat::U8, None), 7);
    assert_eq!(depth(SampleFormat::S16, None), 15);
    assert_eq!(depth(SampleFormat::S24, None), 23);
    assert_eq!(depth(SampleFormat::S24, Some(20)), 19);
    assert_eq!(depth(SampleFormat::S32, None), 31);
}

#[test]
fn other_codecs_and_malformed_extradata_are_refused() {
    let mut s = stream(&block(0, 0).write());
    s.codec = CodecId::PcmS16Le;
    assert!(matches!(
        create(Box::new(Shared::default()), &s, &Tags::new()).err(),
        Some(Error::Unsupported { .. })
    ));

    let mut m = create(
        Box::new(Shared::default()),
        &stream(&block(0, 0).write()),
        &Tags::new(),
    )
    .unwrap();
    for bad in [&[][..], &[0u8; 33][..], &[0u8; 35][..]] {
        assert!(matches!(m.set_extradata(bad), Err(Error::Malformed { .. })));
    }
    // 34 bytes that are not a STREAMINFO body either.
    assert!(matches!(
        m.set_extradata(&[0u8; 34]),
        Err(Error::Malformed { .. })
    ));
    assert!(matches!(
        create(
            Box::new(Shared::default()),
            &stream(&[0u8; 34]),
            &Tags::new()
        )
        .err(),
        Some(Error::Malformed { .. })
    ));
}

#[test]
fn a_comment_block_over_the_24_bit_length_limit_is_refused() {
    let mut t = Tags::new();
    t.push("COMMENT", "x".repeat(1 << 24));
    let r = create(
        Box::new(Shared::default()),
        &stream(&block(0, 0).write()),
        &t,
    );
    assert!(matches!(r.err(), Some(Error::Unsupported { .. })));
}

const MASK_KEY: &str = "WAVEFORMATEXTENSIBLE_CHANNEL_MASK";

/// The comments a FLAC file for `layout` carries when written with `carried` tags.
fn written_comments(layout: ChannelLayout, carried: &Tags) -> Vec<(String, String)> {
    let audio = AudioSpec::new(48000, SampleFormat::S16, layout).unwrap();
    let s = StreamInfo::new(0, CodecId::Flac, audio, TimeBase::new(1, 48000).unwrap());
    let sink = Shared::default();
    create(Box::new(sink.clone()), &s, carried)
        .unwrap()
        .finish()
        .unwrap();
    let bytes = sink.bytes();
    let parsed = vorbis_comment::parse(&bytes[46..], &Limits::default()).unwrap();
    parsed
        .tags
        .iter()
        .map(|t| (t.key.clone(), t.value.clone()))
        .collect()
}

fn pair(k: &str, v: &str) -> (String, String) {
    (k.to_owned(), v.to_owned())
}

#[test]
fn a_layout_other_than_the_default_writes_one_mask_tag() {
    // 5.1 with back channels: the default for six channels uses the side pair.
    let back = ChannelLayout::from_mask(0x3F, 6).unwrap();
    assert_eq!(
        written_comments(back.clone(), &Tags::new()),
        [pair(MASK_KEY, "0x003F")]
    );
    // Any tag carried over from the source is replaced, whatever its spelling.
    let mut carried = tags();
    carried.push("waveformatextensible_channel_mask", "0x0001");
    carried.push(MASK_KEY, "0x060F");
    assert_eq!(
        written_comments(back, &carried),
        [
            pair("TITLE", "Tone"),
            pair("ARTIST", "A"),
            pair("ARTIST", "B"),
            pair(MASK_KEY, "0x003F"),
        ]
    );
}

#[test]
fn the_mask_is_spelled_like_the_reference_writes_it() {
    // Uppercase hex, at least four digits.
    for (mask, channels, text) in [
        (0x1u32, 1u16, "0x0001"),
        (0x3F, 6, "0x003F"),
        (0xC0, 2, "0x00C0"),
        (0x3F003, 8, "0x3F003"),
    ] {
        let layout = ChannelLayout::from_mask(mask, channels).unwrap();
        assert_eq!(written_comments(layout, &Tags::new())[0].1, text);
    }
}

#[test]
fn the_default_layout_writes_no_tag_and_drops_a_stale_one() {
    for (mask, channels) in [
        (0x4u32, 1u16),
        (0x3, 2),
        (0x7, 3),
        (0x33, 4),
        (0x607, 5),
        (0x60F, 6),
        (0x70F, 7),
        (0x63F, 8),
    ] {
        let layout = ChannelLayout::from_mask(mask, channels).unwrap();
        let mut stale = tags();
        stale.push(MASK_KEY, "0x0001");
        assert_eq!(
            written_comments(layout, &stale),
            [
                pair("TITLE", "Tone"),
                pair("ARTIST", "A"),
                pair("ARTIST", "B")
            ],
            "{mask:#x}"
        );
    }
}

#[test]
fn a_layout_without_positions_writes_no_tag() {
    let layout = ChannelLayout::unpositioned(6).unwrap();
    let mut stale = Tags::new();
    stale.push(MASK_KEY, "0x003F");
    assert_eq!(written_comments(layout, &stale), []);
}
