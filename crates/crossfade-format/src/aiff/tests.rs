//! Reader tests. The builders here know nothing about the reader or the writer.

use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Demuxer, Error, Limits, Packet, SampleFormat, TimeBase,
};
use crossfade_testkit::io::FaultySource;
use std::io::Cursor;

const EXT_44100: [u8; 10] = [0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0];
const EXT_8000: [u8; 10] = [0x40, 0x0B, 0xFA, 0, 0, 0, 0, 0, 0, 0];

fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut c = raw_chunk(id, body.len() as i32, body);
    if body.len() % 2 == 1 {
        c.push(0);
    }
    c
}

fn raw_chunk(id: &[u8; 4], declared: i32, body: &[u8]) -> Vec<u8> {
    let mut c = id.to_vec();
    c.extend_from_slice(&declared.to_be_bytes());
    c.extend_from_slice(body);
    c
}

fn form(kind: &[u8; 4], chunks: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = chunks.concat();
    let mut f = b"FORM".to_vec();
    f.extend_from_slice(&(body.len() as u32 + 4).to_be_bytes());
    f.extend_from_slice(kind);
    f.extend_from_slice(&body);
    f
}

fn comm_body(channels: i16, frames: u32, bits: i16, rate: [u8; 10]) -> Vec<u8> {
    let mut b = channels.to_be_bytes().to_vec();
    b.extend_from_slice(&frames.to_be_bytes());
    b.extend_from_slice(&bits.to_be_bytes());
    b.extend_from_slice(&rate);
    b
}

fn comm(channels: i16, frames: u32, bits: i16) -> Vec<u8> {
    chunk(b"COMM", &comm_body(channels, frames, bits, EXT_44100))
}

fn comm_c(channels: i16, frames: u32, bits: i16, comp: &[u8; 4], name: &str) -> Vec<u8> {
    let mut b = comm_body(channels, frames, bits, EXT_44100);
    b.extend_from_slice(comp);
    b.push(name.len() as u8);
    b.extend_from_slice(name.as_bytes());
    if (1 + name.len()) % 2 == 1 {
        b.push(0);
    }
    chunk(b"COMM", &b)
}

fn ssnd_at(offset: u32, data: &[u8]) -> Vec<u8> {
    let mut b = offset.to_be_bytes().to_vec();
    b.extend_from_slice(&0u32.to_be_bytes());
    b.extend_from_slice(data);
    chunk(b"SSND", &b)
}

fn ssnd(data: &[u8]) -> Vec<u8> {
    ssnd_at(0, data)
}

fn aiff(channels: i16, bits: i16, data: &[u8], bytes_per_frame: usize) -> Vec<u8> {
    let frames = (data.len() / bytes_per_frame) as u32;
    form(b"AIFF", &[comm(channels, frames, bits), ssnd(data)])
}

fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + 3) as u8).collect()
}

fn open_with(bytes: Vec<u8>, limits: &Limits) -> Result<Box<dyn Demuxer>, Error> {
    super::open(Box::new(Cursor::new(bytes)), limits)
}

fn open(bytes: Vec<u8>) -> Result<Box<dyn Demuxer>, Error> {
    open_with(bytes, &Limits::default())
}

fn open_pipe(bytes: Vec<u8>) -> Result<Box<dyn Demuxer>, Error> {
    let src = FaultySource::new(Cursor::new(bytes)).not_seekable();
    super::open(Box::new(src), &Limits::default())
}

type Got = (i64, u64, Vec<u8>);

fn drain(d: &mut dyn Demuxer) -> (Vec<Got>, Option<Error>) {
    let mut pkt = Packet::default();
    let mut out = Vec::new();
    loop {
        match d.read_packet(&mut pkt) {
            Ok(true) => out.push((pkt.pts, pkt.duration, pkt.data.clone())),
            Ok(false) => return (out, None),
            Err(e) => return (out, Some(e)),
        }
    }
}

fn all_data(d: &mut dyn Demuxer) -> Vec<u8> {
    let (packets, err) = drain(d);
    assert_eq!(err, None);
    packets.into_iter().flat_map(|p| p.2).collect()
}

fn tags(d: &dyn Demuxer) -> Vec<(String, String)> {
    d.tags()
        .iter()
        .map(|t| (t.key.clone(), t.value.clone()))
        .collect()
}

fn unsupported(r: Result<Box<dyn Demuxer>, Error>) -> String {
    match r {
        Err(Error::Unsupported { what }) => what,
        Err(e) => panic!("wrong error {e:?}"),
        Ok(_) => panic!("expected Unsupported"),
    }
}

fn malformed(r: Result<Box<dyn Demuxer>, Error>) -> String {
    match r {
        Err(Error::Malformed { what }) => what,
        Err(e) => panic!("wrong error {e:?}"),
        Ok(_) => panic!("expected Malformed"),
    }
}

#[test]
fn every_depth_class_maps_to_a_codec_and_keeps_the_bytes() {
    use CodecId::*;
    use SampleFormat as F;
    // (sampleSize, codec, engine format, bits_per_raw_sample, bytes per sample)
    let cases = [
        (1, PcmS8, F::U8, Some(1), 1),
        (8, PcmS8, F::U8, None, 1),
        (9, PcmS16Be, F::S16, Some(9), 2),
        (12, PcmS16Be, F::S16, Some(12), 2),
        (16, PcmS16Be, F::S16, None, 2),
        (17, PcmS24Be, F::S24, Some(17), 3),
        (20, PcmS24Be, F::S24, Some(20), 3),
        (24, PcmS24Be, F::S24, None, 3),
        (25, PcmS32Be, F::S32, Some(25), 4),
        (32, PcmS32Be, F::S32, None, 4),
    ];
    for (bits, codec, format, raw, size) in cases {
        let pcm = payload(10 * 2 * size);
        let mut d = open(aiff(2, bits, &pcm, 2 * size)).unwrap();
        let s = &d.streams()[0];
        assert_eq!(s.index, 0);
        assert_eq!(s.codec, codec, "{bits}");
        assert_eq!(
            s.audio,
            AudioSpec::new(44100, format, ChannelLayout::stereo()).unwrap()
        );
        assert_eq!(s.bits_per_raw_sample, raw, "{bits}");
        assert_eq!(s.time_base, TimeBase::new(1, 44100).unwrap());
        assert_eq!(s.frames, Some(10));
        assert!(s.extradata.is_empty());
        assert_eq!(drain(d.as_mut()), (vec![(0, 10, pcm)], None), "{bits}");
    }
}

#[test]
fn aifc_compression_types_that_are_plain_pcm() {
    use CodecId::*;
    // (type, sampleSize, codec, bytes per sample)
    let cases: [(&[u8; 4], i16, CodecId, usize); 13] = [
        (b"NONE", 16, PcmS16Be, 2),
        (b"twos", 16, PcmS16Be, 2),
        (b"twos", 8, PcmS8, 1),
        (b"in24", 24, PcmS24Be, 3),
        (b"in32", 32, PcmS32Be, 4),
        (b"sowt", 16, PcmS16Le, 2),
        (b"sowt", 24, PcmS24Le, 3),
        (b"sowt", 32, PcmS32Le, 4),
        (b"sowt", 8, PcmS8, 1),
        (b"raw ", 8, PcmU8, 1),
        (b"fl32", 32, PcmF32Be, 4),
        (b"FL32", 32, PcmF32Be, 4),
        (b"fl64", 64, PcmF64Be, 8),
    ];
    for (comp, bits, codec, size) in cases {
        let pcm = payload(6 * size);
        let f = form(b"AIFC", &[comm_c(1, 6, bits, comp, "n"), ssnd(&pcm)]);
        let mut d = open(f).unwrap();
        assert_eq!(d.streams()[0].codec, codec, "{comp:?} {bits}");
        assert_eq!(d.streams()[0].bits_per_raw_sample, None, "{comp:?}");
        assert_eq!(all_data(d.as_mut()), pcm);
    }
    let f = form(
        b"AIFC",
        &[comm_c(1, 2, 64, b"FL64", ""), ssnd(&payload(16))],
    );
    assert_eq!(open(f).unwrap().streams()[0].codec, PcmF64Be);
}

#[test]
fn aifc_reduced_depth_keeps_the_raw_sample_size() {
    let f = form(b"AIFC", &[comm_c(1, 2, 12, b"twos", ""), ssnd(&[0; 4])]);
    let d = open(f).unwrap();
    assert_eq!(d.streams()[0].codec, CodecId::PcmS16Be);
    assert_eq!(d.streams()[0].bits_per_raw_sample, Some(12));
}

#[test]
fn other_compression_types_are_unsupported_by_name() {
    for comp in [b"ulaw", b"alaw", b"ima4", b"MAC3", b"G722"] {
        let f = form(b"AIFC", &[comm_c(1, 2, 16, comp, "x"), ssnd(&[0; 4])]);
        let what = unsupported(open(f));
        assert!(what.contains(std::str::from_utf8(comp).unwrap()), "{what}");
    }
}

#[test]
fn floats_and_unsigned_need_the_right_sample_size() {
    for (comp, bits) in [(b"fl32", 16), (b"fl64", 32), (b"raw ", 16)] {
        let f = form(b"AIFC", &[comm_c(1, 1, bits, comp, ""), ssnd(&[0; 8])]);
        unsupported(open(f));
    }
}

#[test]
fn sample_size_out_of_range() {
    for bits in [0i16, -8] {
        malformed(open(aiff(1, bits, &[0; 4], 2)));
    }
    unsupported(open(aiff(1, 33, &[0; 8], 5)));
    unsupported(open(aiff(1, 64, &[0; 8], 8)));
}

#[test]
fn channel_layouts() {
    let layout = |ch| {
        let d = open(aiff(ch, 16, &[], 2)).unwrap();
        d.streams()[0].audio.layout().clone()
    };
    assert_eq!(layout(1), ChannelLayout::mono());
    assert_eq!(layout(2), ChannelLayout::stereo());
    assert_eq!(layout(6), ChannelLayout::unpositioned(6).unwrap());
    malformed(open(aiff(0, 16, &[], 2)));
    malformed(open(aiff(-1, 16, &[], 2)));
}

#[test]
fn the_sample_rate_is_decoded_exactly() {
    let rate = |ext: [u8; 10]| {
        let f = form(
            b"AIFF",
            &[chunk(b"COMM", &comm_body(1, 0, 16, ext)), ssnd(&[])],
        );
        open(f).map(|d| d.streams()[0].audio.sample_rate())
    };
    assert_eq!(rate(EXT_8000).unwrap(), 8000);
    assert_eq!(rate([0x3F, 0xFF, 0x80, 0, 0, 0, 0, 0, 0, 0]).unwrap(), 1);
    assert_eq!(
        rate([0x40, 0x1E, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]).unwrap(),
        u32::MAX
    );
    // 44100.5 Hz
    let frac = [0x40, 0x0E, 0xAC, 0x44, 0x80, 0, 0, 0, 0, 0];
    assert!(matches!(rate(frac), Err(Error::Unsupported { .. })));
    assert!(matches!(rate([0; 10]), Err(Error::Malformed { .. })));
    let neg = [0xC0, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0];
    assert!(matches!(rate(neg), Err(Error::Malformed { .. })));
}

#[test]
fn offset_bytes_are_skipped_before_the_first_frame() {
    let mut body = vec![0xEE, 0xEE, 0xEE];
    body.extend_from_slice(&[1, 2, 3, 4]);
    let f = form(b"AIFF", &[comm(1, 2, 16), ssnd_at(3, &body)]);
    assert_eq!(all_data(open(f).unwrap().as_mut()), [1, 2, 3, 4]);
}

#[test]
fn the_frame_count_comes_from_comm_not_the_chunk_size() {
    let mut body = vec![0u8; 8];
    body.extend_from_slice(&[9, 8, 7, 6]);
    let f = form(b"AIFF", &[comm(1, 2, 16), raw_chunk(b"SSND", 8, &body)]);
    assert_eq!(all_data(open(f).unwrap().as_mut()), [9, 8, 7, 6]);
}

#[test]
fn odd_chunks_are_padded_and_chunks_may_come_in_any_order() {
    let f = form(
        b"AIFF",
        &[
            chunk(b"JUNK", &[1, 2, 3]),
            ssnd(&[1, 2, 3, 4, 5, 6, 7, 8]),
            chunk(b"MARK", &[9]),
            comm(1, 4, 16),
            chunk(b"NAME", b"abc"),
        ],
    );
    let mut d = open(f).unwrap();
    assert_eq!(all_data(d.as_mut()), [1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(tags(d.as_ref()), [("TITLE".into(), "abc".into())]);
}

#[test]
fn an_odd_sized_sound_chunk_is_followed_by_its_pad_byte() {
    let f = form(
        b"AIFF",
        &[comm(1, 3, 8), ssnd(&[1, 2, 3]), chunk(b"NAME", b"after")],
    );
    let mut d = open(f).unwrap();
    assert_eq!(d.streams()[0].codec, CodecId::PcmS8);
    assert_eq!(all_data(d.as_mut()), [1, 2, 3]);
    assert_eq!(tags(d.as_ref()), [("TITLE".into(), "after".into())]);
}

#[test]
fn sound_data_before_comm_needs_a_seekable_input() {
    let f = form(b"AIFF", &[ssnd(&[1, 2, 3, 4]), comm(1, 2, 16)]);
    assert_eq!(
        unsupported(open_pipe(f.clone())),
        "AIFF with sound data before its COMM chunk needs a seekable input"
    );
    assert_eq!(all_data(open(f).unwrap().as_mut()), [1, 2, 3, 4]);
}

#[test]
fn a_pipe_reads_forward_and_misses_text_after_the_sound() {
    let f = form(
        b"AIFF",
        &[
            chunk(b"NAME", b"before"),
            comm(1, 2, 16),
            ssnd(&[1, 2, 3, 4]),
            chunk(b"AUTH", b"after"),
        ],
    );
    let mut pipe = open_pipe(f.clone()).unwrap();
    assert_eq!(all_data(pipe.as_mut()), [1, 2, 3, 4]);
    assert_eq!(tags(pipe.as_ref()), [("TITLE".into(), "before".into())]);
    let seekable = open(f).unwrap();
    assert_eq!(
        tags(seekable.as_ref()),
        [
            ("TITLE".into(), "before".into()),
            ("ARTIST".into(), "after".into())
        ]
    );
}

#[test]
fn text_chunks_become_tags_one_per_occurrence() {
    let f = form(
        b"AIFF",
        &[
            comm(1, 0, 16),
            chunk(b"NAME", b"Song"),
            chunk(b"AUTH", b"Me"),
            chunk(b"(c) ", b"2026 Me"),
            chunk(b"ANNO", b"one"),
            chunk(b"ANNO", b"two"),
            chunk(b"APPL", b"ignored"),
            ssnd(&[]),
        ],
    );
    let d = open(f).unwrap();
    let want = [
        ("TITLE", "Song"),
        ("ARTIST", "Me"),
        ("COPYRIGHT", "2026 Me"),
        ("COMMENT", "one"),
        ("COMMENT", "two"),
    ]
    .map(|(k, v)| (k.to_string(), v.to_string()));
    assert_eq!(tags(d.as_ref()), want);
}

#[test]
fn text_is_utf8_then_latin1_with_trailing_nuls_trimmed() {
    let f = form(
        b"AIFF",
        &[
            comm(1, 0, 16),
            chunk(b"NAME", "Café\0\0".as_bytes()),
            chunk(b"AUTH", &[b'C', b'a', b'f', 0xE9]),
            chunk(b"ANNO", b""),
            ssnd(&[]),
        ],
    );
    let d = open(f).unwrap();
    assert_eq!(
        tags(d.as_ref()),
        [
            ("TITLE".into(), "Café".into()),
            ("ARTIST".into(), "Café".into()),
            ("COMMENT".into(), String::new()),
        ]
    );
}

#[test]
fn an_oversized_text_chunk_is_skipped_not_read() {
    let big = vec![b'x'; 64 * 1024 + 1];
    let f = form(
        b"AIFF",
        &[
            comm(1, 0, 16),
            chunk(b"NAME", &big),
            chunk(b"AUTH", b"ok"),
            ssnd(&[]),
        ],
    );
    assert_eq!(
        tags(open(f).unwrap().as_ref()),
        [("ARTIST".into(), "ok".into())]
    );
}

#[test]
fn text_beyond_the_alloc_limit_is_skipped() {
    let limits = Limits {
        max_alloc_bytes: 8,
        max_packet_bytes: 2,
        ..Limits::default()
    };
    let f = form(
        b"AIFF",
        &[
            comm(1, 0, 16),
            chunk(b"NAME", b"too long for the limit"),
            chunk(b"AUTH", b"fits"),
            ssnd(&[]),
        ],
    );
    let d = open_with(f, &limits).unwrap();
    assert_eq!(tags(d.as_ref()), [("ARTIST".into(), "fits".into())]);
}

#[test]
fn fver_is_accepted_absent_or_odd() {
    let with = |v: u32| {
        form(
            b"AIFC",
            &[
                chunk(b"FVER", &v.to_be_bytes()),
                comm_c(1, 2, 16, b"NONE", "not compressed"),
                ssnd(&[1, 2, 3, 4]),
            ],
        )
    };
    for v in [0xA280_5140, 0, 7] {
        assert_eq!(all_data(open(with(v)).unwrap().as_mut()), [1, 2, 3, 4]);
    }
    let without = form(
        b"AIFC",
        &[comm_c(1, 2, 16, b"NONE", ""), ssnd(&[1, 2, 3, 4])],
    );
    assert_eq!(all_data(open(without).unwrap().as_mut()), [1, 2, 3, 4]);
}

#[test]
fn missing_chunks_and_bad_headers() {
    malformed(open(form(b"AIFF", &[ssnd(&[0; 4])])));
    malformed(open(form(b"AIFF", &[comm(1, 2, 16)])));
    malformed(open(form(b"AIFF", &[])));
    // AIFC needs the compression type: an 18-byte COMM is too short.
    malformed(open(form(b"AIFC", &[comm(1, 0, 16), ssnd(&[])])));
    malformed(open(form(b"AIFF", &[chunk(b"COMM", &[0; 10]), ssnd(&[])])));
    // A sound chunk too short to hold offset and blockSize.
    malformed(open(form(
        b"AIFF",
        &[comm(1, 0, 16), chunk(b"SSND", &[0; 4])],
    )));
    // Negative chunk size.
    let mut f = form(b"AIFF", &[comm(1, 0, 16)]);
    f.extend_from_slice(&raw_chunk(b"JUNK", -2, &[]));
    malformed(open(f));
    assert!(matches!(
        open(b"FORM\0\0\0\x04WAVE".to_vec()),
        Err(Error::UnrecognisedFormat)
    ));
    assert!(matches!(open(Vec::new()), Err(Error::UnrecognisedFormat)));
}

#[test]
fn long_data_splits_into_packets_with_running_pts() {
    let frames = 4096 * 2 + 5;
    let pcm = payload(frames * 2);
    let mut d = open(aiff(1, 16, &pcm, 2)).unwrap();
    let (packets, err) = drain(d.as_mut());
    assert_eq!(err, None);
    let got: Vec<(i64, u64, usize)> = packets.iter().map(|p| (p.0, p.1, p.2.len())).collect();
    assert_eq!(got, [(0, 4096, 8192), (4096, 4096, 8192), (8192, 5, 10)]);
    assert_eq!(packets.concat_data(), pcm);
}

trait ConcatData {
    fn concat_data(&self) -> Vec<u8>;
}

impl ConcatData for Vec<Got> {
    fn concat_data(&self) -> Vec<u8> {
        self.iter().flat_map(|p| p.2.clone()).collect()
    }
}

#[test]
fn a_short_file_yields_the_whole_frames_then_unexpected_eof() {
    // COMM promises 10 stereo 16-bit frames; the file holds 2 and a half.
    let pcm = payload(10);
    let f = form(b"AIFF", &[comm(2, 10, 16), ssnd(&pcm)]);
    let len = f.len() as u64;
    for mut d in [open(f.clone()).unwrap(), open_pipe(f).unwrap()] {
        let mut pkt = Packet::default();
        assert!(d.read_packet(&mut pkt).unwrap());
        assert_eq!(
            (pkt.pts, pkt.duration, pkt.data.as_slice()),
            (0, 2, &pcm[..8])
        );
        assert_eq!(
            d.read_packet(&mut pkt),
            Err(Error::UnexpectedEof { offset: len })
        );
        assert_eq!(d.read_packet(&mut pkt), Ok(false));
    }
}

#[test]
fn zero_frames_is_an_empty_stream() {
    let mut d = open(aiff(2, 16, &[], 4)).unwrap();
    assert_eq!(d.streams()[0].frames, Some(0));
    assert_eq!(drain(d.as_mut()), (vec![], None));
}

#[test]
fn truncation_at_every_byte_never_panics() {
    let f = form(
        b"AIFC",
        &[
            chunk(b"FVER", &0xA280_5140u32.to_be_bytes()),
            chunk(b"NAME", b"t"),
            comm_c(2, 3, 16, b"NONE", "x"),
            ssnd(&payload(12)),
            chunk(b"ANNO", b"tail"),
        ],
    );
    for cut in 0..=f.len() as u64 {
        for seekable in [true, false] {
            let mut src = FaultySource::new(Cursor::new(f.clone())).truncate_at(cut);
            if !seekable {
                src = src.not_seekable();
            }
            if let Ok(mut d) = super::open(Box::new(src), &Limits::default()) {
                let _ = drain(d.as_mut());
            }
        }
    }
}

#[test]
fn hostile_sizes_are_checked_before_any_allocation() {
    let tiny = Limits {
        max_alloc_bytes: 64,
        max_nesting_depth: 4,
        max_packet_bytes: 64,
        ..Limits::default()
    };
    // Huge unknown and text chunks are skipped without being read.
    for id in [b"JUNK", b"NAME", b"COMT"] {
        let mut f = form(b"AIFF", &[comm(1, 0, 16)]);
        f.extend_from_slice(&raw_chunk(id, i32::MAX, &[]));
        malformed(open_with(f.clone(), &tiny)); // no sound chunk before the file ends
        let mut g = form(b"AIFF", &[comm(1, 0, 16), ssnd(&[])]);
        g.extend_from_slice(&raw_chunk(id, i32::MAX, &[]));
        assert!(open_with(g, &tiny).is_ok());
    }
    // A frame that cannot fit a packet.
    let wide = form(b"AIFC", &[comm_c(30_000, 1, 64, b"fl64", ""), ssnd(&[])]);
    assert!(matches!(
        open_with(wide, &tiny),
        Err(Error::AllocLimitExceeded { .. })
    ));
}

#[test]
fn a_small_packet_limit_shrinks_the_packets() {
    let limits = Limits {
        max_packet_bytes: 8,
        ..Limits::default()
    };
    let pcm = payload(20);
    let mut d = open_with(aiff(1, 16, &pcm, 2), &limits).unwrap();
    let (packets, err) = drain(d.as_mut());
    assert_eq!(err, None);
    assert_eq!(packets.iter().map(|p| p.1).collect::<Vec<_>>(), [4, 4, 2]);
}

#[test]
fn open_demuxer_sniffs_aiff_and_aifc_even_on_a_pipe() {
    let plain = aiff(1, 16, &[1, 2, 3, 4], 2);
    let compressed = form(
        b"AIFC",
        &[comm_c(1, 2, 16, b"sowt", ""), ssnd(&[1, 2, 3, 4])],
    );
    for bytes in [plain, compressed] {
        let src = FaultySource::new(Cursor::new(bytes)).not_seekable();
        let (id, mut d) = crate::open_demuxer(Box::new(src), &Limits::default()).unwrap();
        assert_eq!(id, crate::FormatId::Aiff);
        assert_eq!(all_data(d.as_mut()), [1, 2, 3, 4]);
    }
}
