//! The builder here is deliberately independent of the muxer.

use super::*;
use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Demuxer, Error, Limits, Packet, SampleFormat, TimeBase,
};
use std::io::Cursor;

const TAG_PCM: u16 = 1;
const TAG_FLOAT: u16 = 3;

struct Fmt {
    tag: u16,
    channels: u16,
    rate: u32,
    bits: u16,
}

impl Fmt {
    fn new(tag: u16, channels: u16, rate: u32, bits: u16) -> Self {
        Self {
            tag,
            channels,
            rate,
            bits,
        }
    }

    fn block_align(&self) -> u16 {
        self.channels * self.bits / 8
    }

    fn base(&self, tag: u16) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&tag.to_le_bytes());
        b.extend_from_slice(&self.channels.to_le_bytes());
        b.extend_from_slice(&self.rate.to_le_bytes());
        let byte_rate = self.rate.wrapping_mul(u32::from(self.block_align()));
        b.extend_from_slice(&byte_rate.to_le_bytes());
        b.extend_from_slice(&self.block_align().to_le_bytes());
        b.extend_from_slice(&self.bits.to_le_bytes());
        b
    }

    fn body(&self) -> Vec<u8> {
        self.base(self.tag)
    }
}

fn raw_chunk(id: &[u8; 4], declared: u32, body: &[u8]) -> Vec<u8> {
    let mut c = id.to_vec();
    c.extend_from_slice(&declared.to_le_bytes());
    c.extend_from_slice(body);
    c
}

/// Chunk with the RIFF pad byte after an odd body.
fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut c = raw_chunk(id, body.len() as u32, body);
    if body.len() % 2 == 1 {
        c.push(0);
    }
    c
}

fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = chunks.concat();
    let mut f = b"RIFF".to_vec();
    f.extend_from_slice(&(body.len() as u32 + 4).to_le_bytes());
    f.extend_from_slice(b"WAVE");
    f.extend_from_slice(&body);
    f
}

fn simple(fmt: &Fmt, pcm: &[u8]) -> Vec<u8> {
    riff(&[chunk(b"fmt ", &fmt.body()), chunk(b"data", pcm)])
}

fn open(bytes: Vec<u8>) -> Result<WavDemuxer, Error> {
    WavDemuxer::new(Box::new(Cursor::new(bytes)), &Limits::default())
}

type Got = (i64, u64, Vec<u8>);

/// Every packet up to end of stream or the first error.
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

fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + 3) as u8).collect()
}

fn spec(rate: u32, f: SampleFormat, layout: ChannelLayout) -> AudioSpec {
    AudioSpec::new(rate, f, layout).unwrap()
}

#[test]
fn every_supported_format_reports_its_stream_info() {
    let cases = [
        (TAG_PCM, 8, SampleFormat::U8),
        (TAG_PCM, 16, SampleFormat::S16),
        (TAG_PCM, 24, SampleFormat::S24),
        (TAG_PCM, 32, SampleFormat::S32),
        (TAG_FLOAT, 32, SampleFormat::F32),
        (TAG_FLOAT, 64, SampleFormat::F64),
    ];
    for (tag, bits, format) in cases {
        let fmt = Fmt::new(tag, 2, 44100, bits);
        let pcm = payload(10 * usize::from(fmt.block_align()));
        let mut d = open(simple(&fmt, &pcm)).unwrap();
        let s = d.streams();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].index, 0);
        assert_eq!(s[0].codec, CodecId::pcm_for(format));
        assert_eq!(s[0].audio, spec(44100, format, ChannelLayout::stereo()));
        assert_eq!(s[0].time_base, TimeBase::new(1, 44100).unwrap());
        assert_eq!(s[0].frames, Some(10));
        let (packets, err) = drain(&mut d);
        assert_eq!(err, None);
        assert_eq!(packets, vec![(0, 10, pcm)], "{format:?}");
    }
}

#[test]
fn mono_is_front_centre_and_many_channels_are_unpositioned() {
    let d = open(simple(&Fmt::new(TAG_PCM, 1, 8000, 16), &[])).unwrap();
    assert_eq!(d.streams()[0].audio.layout(), &ChannelLayout::mono());
    let d = open(simple(&Fmt::new(TAG_PCM, 3, 8000, 16), &[])).unwrap();
    assert_eq!(
        d.streams()[0].audio.layout(),
        &ChannelLayout::unpositioned(3).unwrap()
    );
}

#[test]
fn long_data_splits_into_packets_with_running_pts() {
    let fmt = Fmt::new(TAG_PCM, 1, 8000, 8);
    let frames = FRAMES_PER_PACKET * 2 + 5;
    let pcm = payload(frames);
    let mut d = open(simple(&fmt, &pcm)).unwrap();
    let (packets, err) = drain(&mut d);
    assert_eq!(err, None);
    let shape: Vec<_> = packets.iter().map(|p| (p.0, p.1)).collect();
    let n = FRAMES_PER_PACKET as i64;
    assert_eq!(shape, [(0, n as u64), (n, n as u64), (2 * n, 5)]);
    let joined: Vec<u8> = packets.into_iter().flat_map(|p| p.2).collect();
    assert_eq!(joined, pcm);
}

#[test]
fn packet_buffer_is_reused() {
    let fmt = Fmt::new(TAG_PCM, 1, 8000, 8);
    let mut d = open(simple(&fmt, &payload(FRAMES_PER_PACKET * 2))).unwrap();
    let mut pkt = Packet::default();
    assert!(d.read_packet(&mut pkt).unwrap());
    let ptr = pkt.data.as_ptr();
    assert!(d.read_packet(&mut pkt).unwrap());
    assert_eq!(pkt.data.as_ptr(), ptr);
    assert!(!d.read_packet(&mut pkt).unwrap());
}

fn ext_body(fmt: &Fmt, valid: u16, mask: u32, sub_tag: u16) -> Vec<u8> {
    let mut b = fmt.base(0xFFFE);
    b.extend_from_slice(&22u16.to_le_bytes());
    b.extend_from_slice(&valid.to_le_bytes());
    b.extend_from_slice(&mask.to_le_bytes());
    b.extend_from_slice(&sub_tag.to_le_bytes());
    // Rest of the KSDATAFORMAT_SUBTYPE GUID.
    b.extend_from_slice(&[
        0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
    ]);
    b
}

fn ext_file(fmt: &Fmt, valid: u16, mask: u32, sub_tag: u16, pcm: &[u8]) -> Vec<u8> {
    riff(&[
        chunk(b"fmt ", &ext_body(fmt, valid, mask, sub_tag)),
        chunk(b"data", pcm),
    ])
}

#[test]
fn list_chunk_before_data_is_skipped() {
    let fmt = Fmt::new(TAG_PCM, 1, 8000, 16);
    let pcm = payload(8);
    let bytes = riff(&[
        chunk(b"fmt ", &fmt.body()),
        chunk(b"LIST", b"INFOISFT\x05\0\0\0Lavf\0"),
        chunk(b"data", &pcm),
    ]);
    let mut d = open(bytes).unwrap();
    assert_eq!(drain(&mut d), (vec![(0, 4, pcm)], None));
}

#[test]
fn odd_sized_chunk_is_followed_by_a_pad_byte() {
    let fmt = Fmt::new(TAG_PCM, 1, 8000, 8);
    let pcm = payload(6);
    let bytes = riff(&[
        chunk(b"junk", b"abc"),
        chunk(b"fmt ", &fmt.body()),
        chunk(b"fact", &[1, 2, 3, 4, 5]),
        chunk(b"data", &pcm),
    ]);
    let mut d = open(bytes).unwrap();
    assert_eq!(drain(&mut d), (vec![(0, 6, pcm)], None));
}

#[test]
fn chunks_after_data_are_ignored() {
    let fmt = Fmt::new(TAG_PCM, 1, 8000, 8);
    let pcm = payload(5);
    let bytes = riff(&[
        chunk(b"fmt ", &fmt.body()),
        chunk(b"data", &pcm),
        chunk(b"LIST", b"whatever"),
        b"garbage".to_vec(),
    ]);
    let mut d = open(bytes).unwrap();
    assert_eq!(drain(&mut d), (vec![(0, 5, pcm)], None));
}

#[test]
fn extensible_with_a_usable_mask_gives_positions() {
    use crossfade_core::Channel::*;
    // FL FR FC LFE BL BR: 0x3F, in bit order.
    let fmt = Fmt::new(TAG_PCM, 6, 48000, 24);
    let bytes = ext_file(&fmt, 24, 0x3F, 1, &payload(18));
    let d = open(bytes).unwrap();
    let want = ChannelLayout::from_positions(vec![
        FrontLeft,
        FrontRight,
        FrontCenter,
        Lfe,
        BackLeft,
        BackRight,
    ])
    .unwrap();
    assert_eq!(d.streams()[0].audio.layout(), &want);
    assert_eq!(d.streams()[0].audio.format(), SampleFormat::S24);
    // Side channels sit at bits 9 and 10.
    let fmt = Fmt::new(TAG_PCM, 2, 48000, 16);
    let d = open(ext_file(&fmt, 16, (1 << 9) | (1 << 10), 1, &[])).unwrap();
    let want = ChannelLayout::from_positions(vec![SideLeft, SideRight]).unwrap();
    assert_eq!(d.streams()[0].audio.layout(), &want);
}

#[test]
fn extensible_float_uses_the_subformat_tag() {
    let fmt = Fmt::new(0xFFFE, 2, 48000, 32);
    let d = open(ext_file(&fmt, 32, 3, TAG_FLOAT, &payload(8))).unwrap();
    assert_eq!(d.streams()[0].audio.format(), SampleFormat::F32);
}

#[test]
fn extensible_without_a_usable_mask_is_unpositioned() {
    let fmt = Fmt::new(TAG_PCM, 2, 48000, 16);
    for mask in [0u32, 1, 7, 1 << 20 | 1 << 21, 0x80000003] {
        let d = open(ext_file(&fmt, 16, mask, 1, &[])).unwrap();
        assert_eq!(
            d.streams()[0].audio.layout(),
            &ChannelLayout::unpositioned(2).unwrap(),
            "mask {mask:#x}"
        );
    }
}

#[test]
fn streaming_size_reads_until_end_of_input() {
    let fmt = Fmt::new(TAG_PCM, 2, 8000, 16);
    let pcm = payload(4 * 3 + 2);
    let bytes = riff(&[
        chunk(b"fmt ", &fmt.body()),
        raw_chunk(b"data", 0xFFFF_FFFF, &pcm),
    ]);
    let mut d = open(bytes).unwrap();
    assert_eq!(d.streams()[0].frames, None);
    // The trailing half frame is dropped.
    assert_eq!(drain(&mut d), (vec![(0, 3, pcm[..12].to_vec())], None));
}

#[test]
fn streaming_size_spans_several_packets() {
    let fmt = Fmt::new(TAG_PCM, 1, 8000, 8);
    let frames = FRAMES_PER_PACKET + 10;
    let pcm = payload(frames);
    let bytes = riff(&[
        chunk(b"fmt ", &fmt.body()),
        raw_chunk(b"data", 0xFFFF_FFFF, &pcm),
    ]);
    let (packets, err) = drain(&mut open(bytes).unwrap());
    assert_eq!(err, None);
    let n = FRAMES_PER_PACKET;
    assert_eq!(packets.len(), 2);
    assert_eq!((packets[1].0, packets[1].1), (n as i64, 10));
}

#[test]
fn empty_data_is_an_empty_stream() {
    let fmt = Fmt::new(TAG_PCM, 2, 8000, 16);
    let mut d = open(simple(&fmt, &[])).unwrap();
    assert_eq!(d.streams()[0].frames, Some(0));
    assert_eq!(drain(&mut d), (vec![], None));
}

#[test]
fn trailing_partial_frame_in_a_sized_chunk_is_ignored() {
    let fmt = Fmt::new(TAG_PCM, 2, 8000, 16);
    let pcm = payload(4 * 2 + 3);
    let mut d = open(simple(&fmt, &pcm)).unwrap();
    assert_eq!(d.streams()[0].frames, Some(2));
    assert_eq!(drain(&mut d), (vec![(0, 2, pcm[..8].to_vec())], None));
}

fn open_with(bytes: Vec<u8>, limits: &Limits) -> Result<WavDemuxer, Error> {
    WavDemuxer::new(Box::new(Cursor::new(bytes)), limits)
}

fn unsupported(r: Result<WavDemuxer, Error>, needle: &str) {
    match r {
        Err(Error::Unsupported { what }) => {
            assert!(what.contains(needle), "{what:?} lacks {needle:?}");
        }
        other => panic!(
            "expected Unsupported({needle}), got {:?}",
            other.map(|_| ())
        ),
    }
}

fn is_malformed(r: Result<WavDemuxer, Error>) {
    assert!(
        matches!(r, Err(Error::Malformed { .. })),
        "expected Malformed, got {:?}",
        r.map(|_| ())
    );
}

#[test]
fn rf64_and_bw64_are_unsupported() {
    for magic in [b"RF64", b"BW64"] {
        let mut bytes = simple(&Fmt::new(TAG_PCM, 1, 8000, 16), &[]);
        bytes[..4].copy_from_slice(magic);
        unsupported(open(bytes), "RF64");
    }
}

#[test]
fn other_magic_is_unrecognised() {
    for bytes in [
        b"RIFX\0\0\0\0WAVE".to_vec(),
        b"RIFF\0\0\0\0AVI ".to_vec(),
        b"ID3\x03\0\0\0\0\0\0\0\0".to_vec(),
        b"RIFF".to_vec(),
        Vec::new(),
    ] {
        assert_eq!(open(bytes).err(), Some(Error::UnrecognisedFormat));
    }
}

#[test]
fn data_before_fmt_is_malformed() {
    let fmt = Fmt::new(TAG_PCM, 1, 8000, 16);
    let bytes = riff(&[chunk(b"data", &[0, 0]), chunk(b"fmt ", &fmt.body())]);
    is_malformed(open(bytes));
}

#[test]
fn invalid_fmt_fields_are_malformed() {
    let good = Fmt::new(TAG_PCM, 2, 8000, 16);
    let mut zero_ch = Fmt::new(TAG_PCM, 0, 8000, 16);
    zero_ch.channels = 0;
    for fmt in [zero_ch, Fmt::new(TAG_PCM, 2, 0, 16)] {
        is_malformed(open(simple(&fmt, &[])));
    }
    // block_align must equal channels * bytes per sample.
    let mut body = good.body();
    body[12..14].copy_from_slice(&3u16.to_le_bytes());
    is_malformed(open(riff(&[chunk(b"fmt ", &body), chunk(b"data", &[])])));
    // Short fmt chunk.
    let body = &good.body()[..12];
    is_malformed(open(riff(&[chunk(b"fmt ", body), chunk(b"data", &[])])));
    // Extensible fmt without its extension.
    let mut body = good.base(0xFFFE);
    body.extend_from_slice(&[0, 0]);
    is_malformed(open(riff(&[chunk(b"fmt ", &body), chunk(b"data", &[])])));
    // No data chunk at all.
    is_malformed(open(riff(&[chunk(b"fmt ", &good.body())])));
}

#[test]
fn unsupported_tags_and_depths_name_what_they_are() {
    let adpcm = Fmt::new(2, 1, 8000, 4);
    unsupported(open(simple(&adpcm, &[])), "tag 2");
    let alaw = Fmt::new(6, 1, 8000, 8);
    unsupported(open(simple(&alaw, &[])), "tag 6");
    unsupported(open(simple(&Fmt::new(TAG_PCM, 1, 8000, 12), &[])), "12");
    unsupported(open(simple(&Fmt::new(TAG_PCM, 1, 8000, 64), &[])), "64");
    unsupported(open(simple(&Fmt::new(TAG_FLOAT, 1, 8000, 16), &[])), "16");
    unsupported(open(simple(&Fmt::new(TAG_FLOAT, 1, 8000, 24), &[])), "24");
}

#[test]
fn extensible_valid_bits_must_match_the_container() {
    let fmt = Fmt::new(TAG_PCM, 2, 48000, 24);
    unsupported(open(ext_file(&fmt, 20, 3, 1, &[])), "valid");
    // An unsupported sub-format is named too.
    unsupported(open(ext_file(&fmt, 24, 3, 0x11, &[])), "0x11");
}

#[test]
fn oversized_fmt_chunk_is_checked_against_limits_before_allocating() {
    let limits = Limits {
        max_alloc_bytes: 1024,
        ..Limits::default()
    };
    let mut body = Fmt::new(TAG_PCM, 1, 8000, 16).body();
    body.resize(2000, 0);
    let bytes = riff(&[chunk(b"fmt ", &body), chunk(b"data", &[])]);
    assert_eq!(
        open_with(bytes, &limits).err(),
        Some(Error::AllocLimitExceeded {
            requested: 2000,
            max: 1024
        })
    );
    // A huge declared size must fail on the declaration, not on memory.
    let bytes = riff(&[raw_chunk(b"fmt ", 0xFFFF_FFF0, &[])]);
    assert!(matches!(
        open_with(bytes, &limits).err(),
        Some(Error::AllocLimitExceeded { .. })
    ));
}

#[test]
fn a_frame_larger_than_the_packet_limit_is_refused() {
    let limits = Limits {
        max_packet_bytes: 100,
        ..Limits::default()
    };
    let fmt = Fmt::new(TAG_FLOAT, 20, 8000, 64);
    assert!(matches!(
        open_with(simple(&fmt, &[]), &limits).err(),
        Some(Error::AllocLimitExceeded { .. })
    ));
}

#[test]
fn packets_shrink_to_fit_the_packet_limit() {
    let limits = Limits {
        max_packet_bytes: 64,
        ..Limits::default()
    };
    let fmt = Fmt::new(TAG_PCM, 2, 8000, 16);
    let pcm = payload(4 * 40);
    let mut d = open_with(simple(&fmt, &pcm), &limits).unwrap();
    let (packets, err) = drain(&mut d);
    assert_eq!(err, None);
    assert!(packets.iter().all(|p| p.2.len() <= 64));
    assert_eq!(packets.concat_bytes(), pcm);
}

trait ConcatBytes {
    fn concat_bytes(&self) -> Vec<u8>;
}

impl ConcatBytes for Vec<Got> {
    fn concat_bytes(&self) -> Vec<u8> {
        self.iter().flat_map(|p| p.2.clone()).collect()
    }
}

mod robustness {
    use super::*;
    use crossfade_core::MediaSource;
    use crossfade_testkit::io::FaultySource;
    use std::io::ErrorKind;

    fn sample_file() -> (Vec<u8>, Vec<u8>) {
        let fmt = Fmt::new(TAG_PCM, 2, 8000, 16);
        let pcm = payload(4 * (FRAMES_PER_PACKET * 2 + 100));
        let bytes = riff(&[
            chunk(b"LIST", b"odd"),
            chunk(b"fmt ", &fmt.body()),
            chunk(b"data", &pcm),
        ]);
        (bytes, pcm)
    }

    fn demux(src: impl MediaSource + 'static) -> Result<WavDemuxer, Error> {
        WavDemuxer::new(Box::new(src), &Limits::default())
    }

    #[test]
    fn one_byte_reads_give_identical_packets() {
        let (bytes, _) = sample_file();
        let plain = drain(&mut open(bytes.clone()).unwrap());
        let src = FaultySource::new(Cursor::new(bytes)).max_read(1);
        assert_eq!(drain(&mut demux(src).unwrap()), plain);
        assert_eq!(plain.0.len(), 3);
    }

    #[test]
    fn works_on_a_non_seekable_source() {
        let (bytes, pcm) = sample_file();
        let src = FaultySource::new(Cursor::new(bytes)).not_seekable();
        let (packets, err) = drain(&mut demux(src).unwrap());
        assert_eq!(err, None);
        assert_eq!(packets.concat_bytes(), pcm);
    }

    #[test]
    fn truncation_mid_data_gives_whole_frames_then_eof_at_the_cut() {
        let (bytes, pcm) = sample_file();
        let data_start = bytes.len() - pcm.len();
        let packet_bytes = 4 * FRAMES_PER_PACKET;
        // Cut inside the second packet, mid-frame.
        let cut = data_start + packet_bytes + 4 * 10 + 3;
        let src = FaultySource::new(Cursor::new(bytes)).truncate_at(cut as u64);
        let (packets, err) = drain(&mut demux(src).unwrap());
        let shape: Vec<_> = packets.iter().map(|p| (p.0, p.1)).collect();
        assert_eq!(
            shape,
            [
                (0, FRAMES_PER_PACKET as u64),
                (FRAMES_PER_PACKET as i64, 10)
            ]
        );
        assert_eq!(packets.concat_bytes(), pcm[..packet_bytes + 40]);
        assert_eq!(err, Some(Error::UnexpectedEof { offset: cut as u64 }));
    }

    #[test]
    fn truncation_on_a_packet_boundary_reports_that_offset() {
        let (bytes, pcm) = sample_file();
        let cut = (bytes.len() - pcm.len() + 4 * FRAMES_PER_PACKET) as u64;
        let src = FaultySource::new(Cursor::new(bytes)).truncate_at(cut);
        let (packets, err) = drain(&mut demux(src).unwrap());
        assert_eq!(packets.len(), 1);
        assert_eq!(err, Some(Error::UnexpectedEof { offset: cut }));
    }

    #[test]
    fn truncation_inside_the_headers_is_an_eof_too() {
        let (bytes, pcm) = sample_file();
        let first_chunk_cut = 12 + 4;
        let in_fmt = 12 + 8 + 4 + 8 + 5;
        for cut in [first_chunk_cut, in_fmt] {
            let src = FaultySource::new(Cursor::new(bytes.clone())).truncate_at(cut as u64);
            assert_eq!(
                demux(src).err(),
                Some(Error::UnexpectedEof { offset: cut as u64 })
            );
        }
        assert!(pcm.len() > 100);
    }

    #[test]
    fn io_errors_surface_as_io_with_the_injected_kind() {
        let (bytes, pcm) = sample_file();
        let io = |kind| Error::Io {
            kind,
            message: std::io::Error::from(kind).to_string(),
        };
        // In the header.
        let src = FaultySource::new(Cursor::new(bytes.clone())).fail_at(10, ErrorKind::BrokenPipe);
        assert_eq!(demux(src).err(), Some(io(ErrorKind::BrokenPipe)));
        // In the data.
        let at = (bytes.len() - pcm.len() + 4 * FRAMES_PER_PACKET + 8) as u64;
        let src = FaultySource::new(Cursor::new(bytes)).fail_at(at, ErrorKind::PermissionDenied);
        let (packets, err) = drain(&mut demux(src).unwrap());
        assert_eq!(packets.len(), 1);
        assert_eq!(err, Some(io(ErrorKind::PermissionDenied)));
    }

    #[test]
    fn after_the_end_further_reads_keep_saying_end() {
        let (bytes, _) = sample_file();
        let mut d = open(bytes).unwrap();
        drain(&mut d);
        let mut p = Packet::default();
        assert!(!d.read_packet(&mut p).unwrap());
    }
}
