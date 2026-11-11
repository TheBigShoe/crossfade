//! Writer tests: byte-exact layouts written out by hand, then the reader as the judge.

use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Error, Limits, MediaSink, MediaSource, Packet, SampleFormat,
    StreamInfo, Tags, TimeBase,
};
use proptest::prelude::*;
use std::io::{Cursor, SeekFrom};
use std::sync::{Arc, Mutex};

/// Sink whose bytes stay readable after the muxer owns it.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Cursor<Vec<u8>>>>);

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

impl Shared {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().get_ref().clone()
    }
}

fn stream(codec: CodecId, rate: u32, layout: ChannelLayout) -> StreamInfo {
    let format = codec.pcm_sample_format().unwrap();
    StreamInfo::new(
        0,
        codec,
        AudioSpec::new(rate, format, layout).unwrap(),
        TimeBase::new(1, i64::from(rate)).unwrap(),
    )
}

fn write(s: &StreamInfo, tags: &Tags, packets: &[&[u8]]) -> Result<Vec<u8>, Error> {
    let sink = Shared::default();
    let mut m = super::create(Box::new(sink.clone()), s, tags)?;
    for p in packets {
        m.write_packet(&Packet {
            data: p.to_vec(),
            ..Packet::default()
        })?;
    }
    m.finish()?;
    Ok(sink.bytes())
}

fn tags(pairs: &[(&str, &str)]) -> Tags {
    let mut t = Tags::new();
    for (k, v) in pairs {
        t.push(*k, *v);
    }
    t
}

const EXT_8000: [u8; 10] = [0x40, 0x0B, 0xFA, 0, 0, 0, 0, 0, 0, 0];

/// FORM + COMM for plain AIFF, sizes filled in from the parts.
fn plain(channels: u16, frames: u32, bits: u16, middle: &[u8], data: &[u8]) -> Vec<u8> {
    let mut comm = b"COMM\0\0\0\x12".to_vec();
    comm.extend_from_slice(&channels.to_be_bytes());
    comm.extend_from_slice(&frames.to_be_bytes());
    comm.extend_from_slice(&bits.to_be_bytes());
    comm.extend_from_slice(&EXT_8000);
    let mut ssnd = b"SSND".to_vec();
    ssnd.extend_from_slice(&(8 + data.len() as u32).to_be_bytes());
    ssnd.extend_from_slice(&[0; 8]);
    ssnd.extend_from_slice(data);
    if data.len() % 2 == 1 {
        ssnd.push(0);
    }
    let body = [&b"AIFF"[..], &comm, middle, &ssnd].concat();
    let mut f = b"FORM".to_vec();
    f.extend_from_slice(&(body.len() as u32).to_be_bytes());
    f.extend_from_slice(&body);
    f
}

#[test]
fn integer_codecs_write_plain_aiff_byte_for_byte() {
    for (codec, bits, data) in [
        (CodecId::PcmS8, 8, vec![0x80, 0x7F, 0x00, 0x01]),
        (
            CodecId::PcmS16Be,
            16,
            vec![0x80, 0, 0x7F, 0xFF, 0, 1, 0xFF, 0xFF],
        ),
        (
            CodecId::PcmS24Be,
            24,
            vec![0x80, 0, 0, 0x7F, 0xFF, 0xFF, 0, 0, 1, 0xFF, 0xFF, 0xFF],
        ),
        (
            CodecId::PcmS32Be,
            32,
            vec![
                0x80, 0, 0, 0, 0x7F, 0xFF, 0xFF, 0xFF, 0, 0, 0, 1, 0xFF, 0xFF, 0xFF, 0xFF,
            ],
        ),
    ] {
        let frames = (data.len() / usize::from(bits / 8)) as u32;
        let s = stream(codec, 8000, ChannelLayout::mono());
        let got = write(&s, &Tags::new(), &[&data]).unwrap();
        assert_eq!(got, plain(1, frames, bits, &[], &data), "{codec:?}");
    }
}

#[test]
fn tags_become_text_chunks_before_the_sound() {
    let s = stream(CodecId::PcmS16Be, 8000, ChannelLayout::mono());
    let t = tags(&[
        ("ARTIST", "Me"),
        ("TITLE", "T"),
        ("TITLE", "ignored second title"),
        ("COMMENT", "one"),
        ("COMMENT", "two"),
        ("COPYRIGHT", "(c)"),
        ("ALBUM", "dropped"),
        ("TRACKNUMBER", "3"),
    ]);
    let got = write(&s, &t, &[&[0, 1, 0, 2]]).unwrap();
    let mut middle = Vec::new();
    for (id, text) in [
        (b"NAME", &b"T"[..]),
        (b"AUTH", b"Me"),
        (b"(c) ", b"(c)"),
        (b"ANNO", b"one"),
        (b"ANNO", b"two"),
    ] {
        middle.extend_from_slice(id);
        middle.extend_from_slice(&(text.len() as u32).to_be_bytes());
        middle.extend_from_slice(text);
        if text.len() % 2 == 1 {
            middle.push(0);
        }
    }
    assert_eq!(got, plain(1, 2, 16, &middle, &[0, 1, 0, 2]));
}

#[test]
fn an_odd_data_length_gets_a_pad_byte_the_size_does_not_count() {
    let s = stream(CodecId::PcmS8, 8000, ChannelLayout::mono());
    let got = write(&s, &Tags::new(), &[&[1, 2, 3]]).unwrap();
    assert_eq!(got, plain(1, 3, 8, &[], &[1, 2, 3]));
    assert_eq!(got.len() % 2, 0);
    // Also when the odd length is split across packets.
    assert_eq!(write(&s, &Tags::new(), &[&[1], &[2, 3]]).unwrap(), got);
}

#[test]
fn an_empty_file_is_valid() {
    let s = stream(CodecId::PcmS16Be, 8000, ChannelLayout::stereo());
    assert_eq!(
        write(&s, &Tags::new(), &[]).unwrap(),
        plain(2, 0, 16, &[], &[])
    );
}

#[test]
fn floats_write_aifc_with_fver_and_a_compression_name() {
    for (codec, kind, name, bits, data) in [
        (
            CodecId::PcmF32Be,
            b"fl32",
            &b"32-bit floating point"[..],
            32u16,
            vec![0x3F, 0x80, 0, 0, 0xBF, 0x80, 0, 0],
        ),
        (
            CodecId::PcmF64Be,
            b"fl64",
            &b"64-bit floating point"[..],
            64,
            vec![0x3F, 0xF0, 0, 0, 0, 0, 0, 0],
        ),
    ] {
        let s = stream(codec, 8000, ChannelLayout::mono());
        let got = write(&s, &Tags::new(), &[&data]).unwrap();
        let frames = (data.len() / usize::from(bits / 8)) as u32;
        let mut comm = b"COMM".to_vec();
        let body_len = 22 + 1 + name.len() + (1 + name.len()) % 2;
        comm.extend_from_slice(&(body_len as u32).to_be_bytes());
        comm.extend_from_slice(&1u16.to_be_bytes());
        comm.extend_from_slice(&frames.to_be_bytes());
        comm.extend_from_slice(&bits.to_be_bytes());
        comm.extend_from_slice(&EXT_8000);
        comm.extend_from_slice(kind);
        comm.push(name.len() as u8);
        comm.extend_from_slice(name);
        if (1 + name.len()) % 2 == 1 {
            comm.push(0);
        }
        let mut ssnd = b"SSND".to_vec();
        ssnd.extend_from_slice(&(8 + data.len() as u32).to_be_bytes());
        ssnd.extend_from_slice(&[0; 8]);
        ssnd.extend_from_slice(&data);
        let body = [
            &b"AIFC"[..],
            b"FVER\0\0\0\x04\xA2\x80\x51\x40",
            &comm,
            &ssnd,
        ]
        .concat();
        let mut want = b"FORM".to_vec();
        want.extend_from_slice(&(body.len() as u32).to_be_bytes());
        want.extend_from_slice(&body);
        assert_eq!(got, want, "{codec:?}");
    }
}

#[test]
fn other_codecs_are_refused_by_name() {
    for codec in [
        CodecId::PcmS16Le,
        CodecId::PcmU8,
        CodecId::PcmF32Le,
        CodecId::Flac,
    ] {
        let format = codec.pcm_sample_format().unwrap_or(SampleFormat::S16);
        let s = StreamInfo::new(
            0,
            codec,
            AudioSpec::new(8000, format, ChannelLayout::mono()).unwrap(),
            TimeBase::new(1, 8000).unwrap(),
        );
        match super::create(Box::new(Shared::default()), &s, &Tags::new()) {
            Err(Error::Unsupported { what }) => {
                assert_eq!(what, format!("aiff cannot hold {}", codec.name()));
            }
            other => panic!("{codec:?}: {:?}", other.err()),
        }
    }
}

#[test]
fn a_codec_that_disagrees_with_the_sample_format_is_malformed() {
    let s = StreamInfo {
        audio: AudioSpec::new(8000, SampleFormat::S24, ChannelLayout::mono()).unwrap(),
        ..stream(CodecId::PcmS16Be, 8000, ChannelLayout::mono())
    };
    assert!(matches!(
        super::create(Box::new(Shared::default()), &s, &Tags::new()),
        Err(Error::Malformed { .. })
    ));
}

#[test]
fn more_channels_than_a_signed_16_bit_field_holds_are_unsupported() {
    let s = stream(
        CodecId::PcmS8,
        8000,
        ChannelLayout::unpositioned(40_000).unwrap(),
    );
    assert!(matches!(
        super::create(Box::new(Shared::default()), &s, &Tags::new()),
        Err(Error::Unsupported { .. })
    ));
}

#[test]
fn a_partial_frame_is_malformed() {
    let s = stream(CodecId::PcmS16Be, 8000, ChannelLayout::stereo());
    let err = write(&s, &Tags::new(), &[&[0, 1, 2]]).unwrap_err();
    assert!(matches!(err, Error::Malformed { .. }), "{err:?}");
}

#[test]
fn oversized_text_is_dropped_so_the_reader_would_not_have_to_skip_it() {
    let s = stream(CodecId::PcmS8, 8000, ChannelLayout::mono());
    let big = "x".repeat(64 * 1024 + 1);
    let t = tags(&[("TITLE", &big), ("ARTIST", "kept")]);
    let got = write(&s, &t, &[&[0, 0]]).unwrap();
    assert!(got.len() < 200);
    let d = super::open(Box::new(Cursor::new(got)), &Limits::default()).unwrap();
    let all: Vec<_> = d.tags().iter().map(|t| t.key.as_str()).collect();
    assert_eq!(all, ["ARTIST"]);
}

/// Seeks fail, as on a pipe.
struct PipeSink(Arc<Mutex<Vec<u8>>>);

impl MediaSink for PipeSink {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(())
    }
    fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "not seekable",
        ))
    }
    fn finish(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_sink_that_cannot_seek_gets_the_data_and_then_an_io_error_at_finish() {
    // Same as the WAV writer: sizes cannot be patched, so finish reports it.
    let s = stream(CodecId::PcmS16Be, 8000, ChannelLayout::mono());
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut m = super::create(Box::new(PipeSink(bytes.clone())), &s, &Tags::new()).unwrap();
    m.write_packet(&Packet {
        data: vec![1, 2, 3, 4],
        ..Packet::default()
    })
    .unwrap();
    assert!(matches!(m.finish(), Err(Error::Io { .. })));
    let written = bytes.lock().unwrap().clone();
    assert!(written.starts_with(b"FORM"));
    assert!(written.ends_with(&[1, 2, 3, 4]));
}

struct NullSink;

impl MediaSink for NullSink {
    fn write_all(&mut self, _: &[u8]) -> std::io::Result<()> {
        Ok(())
    }
    fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
        Ok(0)
    }
    fn finish(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn sound_data_beyond_a_signed_32_bit_chunk_size_is_unsupported() {
    let s = stream(CodecId::PcmS8, 8000, ChannelLayout::mono());
    let mut m = super::create(Box::new(NullSink), &s, &Tags::new()).unwrap();
    let p = Packet {
        data: vec![0; 64 << 20],
        ..Packet::default()
    };
    let mut written = 0u64;
    let err = loop {
        match m.write_packet(&p) {
            Ok(()) => written += p.data.len() as u64,
            Err(e) => break e,
        }
        assert!(written < 5 << 30, "never refused");
    };
    assert!(matches!(&err, Error::Unsupported { what } if what.contains("2 GiB")));
    assert!(written > (2u64 << 30) - (128 << 20));
}

fn codec_of(i: usize) -> CodecId {
    [
        CodecId::PcmS8,
        CodecId::PcmS16Be,
        CodecId::PcmS24Be,
        CodecId::PcmS32Be,
        CodecId::PcmF32Be,
        CodecId::PcmF64Be,
    ][i]
}

fn read_back(bytes: Vec<u8>) -> (StreamInfo, Vec<u8>, Vec<(String, String)>) {
    let mut d = super::open(Box::new(Cursor::new(bytes)), &Limits::default()).unwrap();
    let mut pkt = Packet::default();
    let mut data = Vec::new();
    while d.read_packet(&mut pkt).unwrap() {
        data.extend_from_slice(&pkt.data);
    }
    let tags = d
        .tags()
        .iter()
        .map(|t| (t.key.clone(), t.value.clone()))
        .collect();
    (d.streams()[0].clone(), data, tags)
}

proptest! {
    #[test]
    fn the_reader_reads_back_what_the_writer_wrote(
        ci in 0..6usize,
        channels in 1u16..=8,
        rate in prop_oneof![Just(1u32), Just(8000), Just(44100), Just(u32::MAX), 1u32..400_000],
        frames in prop_oneof![Just(0usize), Just(4096), Just(4097), 0usize..60],
        seed in any::<u8>(),
        with_tags in any::<bool>(),
        split in 0usize..8,
    ) {
        let codec = codec_of(ci);
        let layout = match channels {
            1 => ChannelLayout::mono(),
            2 => ChannelLayout::stereo(),
            n => ChannelLayout::unpositioned(n).unwrap(),
        };
        let s = stream(codec, rate, layout);
        let len = frames * s.audio.bytes_per_frame();
        let data: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect();
        let t = if with_tags {
            tags(&[("COMMENT", "a"), ("TITLE", "Tïtle"), ("ARTIST", "x"), ("COMMENT", "b"), ("GENRE", "g")])
        } else {
            Tags::new()
        };
        // Split at a frame boundary to exercise several packets.
        let cut = (split * s.audio.bytes_per_frame()).min(len);
        let bytes = write(&s, &t, &[&data[..cut], &data[cut..]]).unwrap();
        prop_assert_eq!(bytes.len() % 2, 0);
        let (info, got, got_tags) = read_back(bytes);
        prop_assert_eq!(info.codec, codec);
        prop_assert_eq!(info.audio, s.audio);
        prop_assert_eq!(info.frames, Some(frames as u64));
        prop_assert_eq!(got, data);
        let want: Vec<(String, String)> = if with_tags {
            [("TITLE", "Tïtle"), ("ARTIST", "x"), ("COMMENT", "a"), ("COMMENT", "b")]
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .to_vec()
        } else {
            vec![]
        };
        prop_assert_eq!(got_tags, want);
    }
}

#[test]
fn the_reader_needs_no_seek_to_read_back_a_tagless_file() {
    // Reader-over-pipe agreement: the layout puts COMM before SSND.
    let s = stream(CodecId::PcmS16Be, 44100, ChannelLayout::stereo());
    let bytes = write(&s, &tags(&[("TITLE", "t")]), &[&[0, 1, 0, 2]]).unwrap();
    let src = crossfade_testkit::io::FaultySource::new(Cursor::new(bytes)).not_seekable();
    assert!(!MediaSource::is_seekable(&src));
    let mut d = super::open(Box::new(src), &Limits::default()).unwrap();
    assert_eq!(d.tags().get("TITLE"), Some("t"));
    let mut p = Packet::default();
    assert!(d.read_packet(&mut p).unwrap());
    assert_eq!(p.data, [0, 1, 0, 2]);
}

#[test]
fn comm_sample_size_is_the_true_depth_while_the_byte_width_agrees() {
    // (codec, true depth, sampleSize written, bytes per sample)
    for (codec, depth, written, size) in [
        (CodecId::PcmS8, 6, 6, 1),
        (CodecId::PcmS16Be, 12, 12, 2),
        (CodecId::PcmS24Be, 20, 20, 3),
        (CodecId::PcmS32Be, 28, 28, 4),
        // A depth whose byte width differs from the container's cannot be told to a reader.
        (CodecId::PcmS24Be, 12, 24, 3),
        (CodecId::PcmS16Be, 16, 16, 2),
    ] {
        let mut s = stream(codec, 8000, ChannelLayout::stereo());
        s.bits_per_raw_sample = Some(depth);
        let data = vec![0x40; 2 * size];
        let bytes = write(&s, &Tags::new(), &[&data]).unwrap();
        assert_eq!(bytes, plain(2, 1, written, &[], &data), "{codec:?} {depth}");
    }
}

#[test]
fn a_true_depth_reads_back_from_what_was_written() {
    let mut s = stream(CodecId::PcmS24Be, 8000, ChannelLayout::stereo());
    s.bits_per_raw_sample = Some(20);
    let bytes = write(&s, &Tags::new(), &[&[0; 6]]).unwrap();
    let (_, d) = crate::open_demuxer(Box::new(Cursor::new(bytes)), &Limits::default()).unwrap();
    assert_eq!(d.streams()[0].bits_per_raw_sample, Some(20));
}
