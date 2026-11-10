use super::*;
use crate::flac::streaminfo::StreamInfoBlock;
use crossfade_core::{AudioFrame, ChannelLayout, SampleFormat, Samples};

fn spec(rate: u32, format: SampleFormat, channels: u16) -> AudioSpec {
    AudioSpec::new(rate, format, ChannelLayout::unpositioned(channels).unwrap()).unwrap()
}

fn opts(level: Option<u8>) -> EncodeOptions {
    EncodeOptions {
        compression_level: level,
    }
}

fn unsupported(r: Result<Box<dyn Encoder>, Error>) -> String {
    match r {
        Err(Error::Unsupported { what }) => what,
        Err(e) => panic!("wrong error {e:?}"),
        Ok(_) => panic!("expected Unsupported"),
    }
}

#[test]
fn only_integer_formats_up_to_24_bits_are_accepted() {
    for f in [SampleFormat::U8, SampleFormat::S16, SampleFormat::S24] {
        assert!(encoder(&spec(44100, f, 2), &opts(None)).is_ok(), "{f:?}");
    }
    assert_eq!(
        unsupported(encoder(&spec(44100, SampleFormat::S32, 2), &opts(None))),
        "flac encoding of 32-bit samples is not supported yet; choose --sample-format s24"
    );
    for f in [SampleFormat::F32, SampleFormat::F64] {
        let what = unsupported(encoder(&spec(44100, f, 2), &opts(None)));
        assert!(what.contains("float"), "{what}");
    }
}

#[test]
fn channel_count_rate_and_level_limits_are_named() {
    assert!(encoder(&spec(44100, SampleFormat::S16, 8), &opts(None)).is_ok());
    let what = unsupported(encoder(&spec(44100, SampleFormat::S16, 9), &opts(None)));
    assert!(what.contains("8 channels"), "{what}");

    assert!(encoder(&spec(1, SampleFormat::S16, 1), &opts(None)).is_ok());
    assert!(encoder(&spec(96_000, SampleFormat::S16, 1), &opts(None)).is_ok());
    let what = unsupported(encoder(&spec(96_001, SampleFormat::S16, 1), &opts(None)));
    assert!(what.contains("96000"), "{what}");

    assert!(encoder(&spec(44100, SampleFormat::S16, 2), &opts(Some(8))).is_ok());
    let what = unsupported(encoder(&spec(44100, SampleFormat::S16, 2), &opts(Some(9))));
    assert!(what.contains("0 to 8"), "{what}");
}

#[test]
fn the_provisional_streaminfo_has_the_format_and_blank_totals() {
    let e = encoder(&spec(48000, SampleFormat::S24, 6), &opts(None)).unwrap();
    let b = StreamInfoBlock::parse(e.extradata()).unwrap();
    assert_eq!(
        b,
        StreamInfoBlock {
            min_block_size: 4096,
            max_block_size: 4096,
            min_frame_size: 0,
            max_frame_size: 0,
            sample_rate: 48000,
            channels: 6,
            bits_per_sample: 24,
            total_samples: 0,
            md5: [0; 16],
        }
    );
    let u8e = encoder(&spec(8000, SampleFormat::U8, 1), &opts(None)).unwrap();
    assert_eq!(
        StreamInfoBlock::parse(u8e.extradata())
            .unwrap()
            .bits_per_sample,
        8
    );
}

#[test]
fn a_frame_of_another_spec_is_an_error() {
    let mut e = encoder(&spec(44100, SampleFormat::S16, 2), &opts(None)).unwrap();
    let mut f = AudioFrame::new(&spec(48000, SampleFormat::S16, 2));
    f.samples = Samples::S16(vec![0; 4]);
    assert!(matches!(e.send_frame(&f), Err(Error::Unsupported { .. })));
    let mut f = AudioFrame::new(&spec(44100, SampleFormat::S16, 1));
    f.samples = Samples::S16(vec![0; 4]);
    assert!(matches!(e.send_frame(&f), Err(Error::Unsupported { .. })));
    let mut f = AudioFrame::new(&spec(44100, SampleFormat::S24, 2));
    f.samples = Samples::S24(vec![0; 4]);
    assert!(matches!(e.send_frame(&f), Err(Error::Unsupported { .. })));
}

use crate::flac::crc::crc16;
use crate::flac::frame::parse_header;
use crate::md5::Md5;

/// Deterministic tone plus a little noise, in range for `bits`.
pub(super) fn signal(channels: usize, frames: usize, bits: u32, seed: u32) -> Vec<i32> {
    let amp = f64::from((1u32 << (bits - 1)) - 1) * 0.5;
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(12345);
    let mut out = Vec::with_capacity(channels * frames);
    for t in 0..frames {
        for c in 0..channels {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = f64::from(state >> 20) / 4096.0 - 0.5;
            let phase = t as f64 * (0.03 + 0.011 * c as f64);
            out.push(((phase.sin() + noise * 0.02) * amp) as i32);
        }
    }
    out
}

fn audio_frame(spec: &AudioSpec, samples: &[i32]) -> AudioFrame {
    let mut f = AudioFrame::new(spec);
    f.samples = match spec.format() {
        SampleFormat::U8 => Samples::U8(samples.iter().map(|&v| (v + 128) as u8).collect()),
        SampleFormat::S16 => Samples::S16(samples.iter().map(|&v| v as i16).collect()),
        SampleFormat::S24 => Samples::S24(samples.to_vec()),
        other => panic!("{other:?}"),
    };
    f
}

pub(super) struct Output {
    pub packets: Vec<Packet>,
    pub extradata: Vec<u8>,
}

/// Feeds `samples` in chunks of `chunk` sample frames, draining after each send.
pub(super) fn encode(spec: &AudioSpec, level: Option<u8>, samples: &[i32], chunk: usize) -> Output {
    let mut e = encoder(spec, &opts(level)).unwrap();
    let per = chunk * usize::from(spec.layout().channels());
    let mut packets = Vec::new();
    let mut pump = |e: &mut Box<dyn Encoder>| loop {
        let mut p = Packet::default();
        if !e.receive_packet(&mut p).unwrap() {
            break;
        }
        packets.push(p);
    };
    for piece in samples.chunks(per.max(1)) {
        e.send_frame(&audio_frame(spec, piece)).unwrap();
        pump(&mut e);
    }
    e.drain();
    pump(&mut e);
    let extradata = e.extradata().to_vec();
    Output { packets, extradata }
}

fn bits(format: SampleFormat) -> u32 {
    match format {
        SampleFormat::U8 => 8,
        SampleFormat::S16 => 16,
        SampleFormat::S24 => 24,
        other => panic!("{other:?}"),
    }
}

/// RFC 9639 section 8.2: interleaved, signed, little-endian, ceil(bits/8) bytes each.
fn rfc_md5(bits: u32, samples: &[i32]) -> [u8; 16] {
    let mut m = Md5::new();
    for s in samples {
        m.update(&s.to_le_bytes()[..bits.div_ceil(8) as usize]);
    }
    m.finalize()
}

fn check_stream(spec: &AudioSpec, samples: &[i32], out: &Output, block: usize) {
    let channels = usize::from(spec.layout().channels());
    let depth = bits(spec.format());
    let total = samples.len() / channels;
    let mut sizes = Vec::new();
    let mut covered = 0u64;
    for (i, p) in out.packets.iter().enumerate() {
        let h = parse_header(&p.data).unwrap();
        assert!(!h.variable_block_size);
        assert_eq!(h.number, i as u64);
        assert_eq!(h.channels.channels() as usize, channels);
        assert!(h.bits_per_sample.is_none_or(|b| u32::from(b) == depth));
        assert!(h.sample_rate.is_none_or(|r| r == spec.sample_rate()));
        let want = block.min(total - i * block);
        assert_eq!(h.block_size as usize, want, "frame {i}");
        assert_eq!(p.pts, (i * block) as i64);
        assert_eq!(p.duration, want as u64);
        // The CRC-16 covers everything before it.
        let (body, tail) = p.data.split_at(p.data.len() - 2);
        assert_eq!(crc16(body).to_be_bytes(), tail, "frame {i}");
        sizes.push(p.data.len());
        covered += p.duration;
    }
    assert_eq!(covered, total as u64);
    assert_eq!(out.packets.len(), total.div_ceil(block));

    let b = StreamInfoBlock::parse(&out.extradata).unwrap();
    assert_eq!(b.total_samples, total as u64);
    assert_eq!(b.md5, rfc_md5(depth, samples));
    assert_eq!(
        (b.min_block_size, b.max_block_size),
        (block as u16, block as u16)
    );
    assert_eq!(
        b.min_frame_size as usize,
        sizes.iter().copied().min().unwrap_or(0)
    );
    assert_eq!(
        b.max_frame_size as usize,
        sizes.iter().copied().max().unwrap_or(0)
    );
    assert_eq!(b.sample_rate, spec.sample_rate());
    assert_eq!(usize::from(b.channels), channels);
    assert_eq!(u32::from(b.bits_per_sample), depth);
}

#[test]
fn every_packet_is_a_valid_frame_and_the_final_streaminfo_matches() {
    for format in [SampleFormat::U8, SampleFormat::S16, SampleFormat::S24] {
        for channels in [1u16, 2, 6] {
            let spec = spec(44100, format, channels);
            // Two full blocks and a short tail.
            let samples = signal(usize::from(channels), 4096 * 2 + 777, bits(format), 1);
            let out = encode(&spec, None, &samples, 1000);
            check_stream(&spec, &samples, &out, 4096);
        }
    }
}

#[test]
fn an_empty_input_is_a_valid_zero_length_stream() {
    let spec = spec(44100, SampleFormat::S16, 2);
    let out = encode(&spec, None, &[], 100);
    assert!(out.packets.is_empty());
    let b = StreamInfoBlock::parse(&out.extradata).unwrap();
    assert_eq!(b.total_samples, 0);
    assert_eq!((b.min_block_size, b.max_block_size), (4096, 4096));
    assert_eq!((b.min_frame_size, b.max_frame_size), (0, 0));
    // MD5 of no bytes, not the "unknown" all-zero value.
    assert_eq!(b.md5, rfc_md5(16, &[]));
}

#[test]
fn a_stream_shorter_than_one_block_keeps_the_block_size_in_streaminfo() {
    let spec = spec(8000, SampleFormat::S16, 1);
    let samples = signal(1, 10, 16, 2);
    let out = encode(&spec, None, &samples, 10);
    check_stream(&spec, &samples, &out, 4096);
}

use proptest::prelude::*;

fn flat(o: &Output) -> (Vec<Packet>, Vec<u8>) {
    (o.packets.clone(), o.extradata.clone())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn chunking_the_input_changes_no_byte(
        format in prop::sample::select(vec![SampleFormat::U8, SampleFormat::S16, SampleFormat::S24]),
        channels in 1u16..=3,
        frames in 0usize..9000,
        chunk in 1usize..5000,
        seed in any::<u32>(),
    ) {
        let spec = spec(22050, format, channels);
        let samples = signal(usize::from(channels), frames, bits(format), seed);
        let whole = encode(&spec, Some(2), &samples, frames.max(1));
        let pieces = encode(&spec, Some(2), &samples, chunk);
        prop_assert_eq!(flat(&whole), flat(&pieces));
    }
}

#[test]
fn two_encoders_give_identical_bytes() {
    let spec = spec(44100, SampleFormat::S16, 2);
    let samples = signal(2, 20_000, 16, 7);
    assert_eq!(
        flat(&encode(&spec, None, &samples, 3000)),
        flat(&encode(&spec, None, &samples, 3000))
    );
}

#[test]
fn our_bytes_equal_flacenc_s_own_stream_for_the_same_settings() {
    use flacenc::component::BitRepr;
    use flacenc::source::MemSource;
    for (channels, format) in [
        (1u16, SampleFormat::S16),
        (2, SampleFormat::S16),
        (2, SampleFormat::S24),
    ] {
        let spec = spec(44100, format, channels);
        let depth = bits(format) as usize;
        let samples = signal(usize::from(channels), 4096 * 2 + 777, depth as u32, 3);
        let ours = encode(&spec, Some(5), &samples, 5000);

        let config = LEVELS[5].config().unwrap();
        let source = MemSource::from_samples(&samples, usize::from(channels), depth, 44100);
        let mut stream = flacenc::encode_with_fixed_block_size(&config, source, 4096).unwrap();
        // The short tail lowers flacenc's minimum block size; the encoder pins both to 4096.
        stream
            .stream_info_mut()
            .set_block_sizes(4096, 4096)
            .unwrap();
        let mut sink = MemSink::<u8>::new();
        stream.write(&mut sink).unwrap();
        let theirs = sink.into_inner();

        assert_eq!(&theirs[..4], b"fLaC");
        assert_eq!(&theirs[8..42], &ours.extradata[..], "{channels} {format:?}");
        let frames: Vec<u8> = ours.packets.iter().flat_map(|p| p.data.clone()).collect();
        assert_eq!(&theirs[42..], &frames[..], "{channels} {format:?}");
    }
}

#[test]
fn a_24_bit_sample_out_of_range_is_malformed() {
    let spec = spec(44100, SampleFormat::S24, 1);
    let mut e = encoder(&spec, &opts(None)).unwrap();
    let mut f = AudioFrame::new(&spec);
    f.samples = Samples::S24(vec![0, 1 << 23]);
    assert!(matches!(e.send_frame(&f), Err(Error::Malformed { .. })));
}

#[test]
fn a_partial_sample_frame_is_malformed_and_sending_after_drain_is_refused() {
    let spec = spec(44100, SampleFormat::S16, 2);
    let mut e = encoder(&spec, &opts(None)).unwrap();
    let mut f = AudioFrame::new(&spec);
    f.samples = Samples::S16(vec![1, 2, 3]);
    assert!(matches!(e.send_frame(&f), Err(Error::Malformed { .. })));
    e.drain();
    e.drain();
    f.samples = Samples::S16(vec![1, 2]);
    assert!(matches!(e.send_frame(&f), Err(Error::Unsupported { .. })));
}

#[test]
fn at_most_one_block_is_held_back_and_packets_appear_as_blocks_fill() {
    let spec = spec(44100, SampleFormat::S16, 2);
    let mut e = encoder(&spec, &opts(None)).unwrap();
    let samples = signal(2, 4095, 16, 9);
    let mut p = Packet::default();
    e.send_frame(&audio_frame(&spec, &samples)).unwrap();
    assert!(!e.receive_packet(&mut p).unwrap());
    e.send_frame(&audio_frame(&spec, &samples[..2])).unwrap();
    assert!(e.receive_packet(&mut p).unwrap());
    assert_eq!((p.pts, p.duration), (0, 4096));
    assert!(!e.receive_packet(&mut p).unwrap());
}

#[test]
fn the_provisional_header_stays_until_drain_and_the_final_one_differs() {
    let spec = spec(44100, SampleFormat::S16, 2);
    let mut e = encoder(&spec, &opts(None)).unwrap();
    let before = e.extradata().to_vec();
    e.send_frame(&audio_frame(&spec, &signal(2, 5000, 16, 4)))
        .unwrap();
    assert_eq!(e.extradata(), &before[..]);
    e.drain();
    assert_ne!(e.extradata(), &before[..]);
}

#[test]
fn every_level_makes_a_valid_stream_and_more_effort_never_costs_bytes_on_a_tone() {
    let spec = spec(44100, SampleFormat::S16, 2);
    let samples = signal(2, 44100, 16, 11);
    let size = |level| {
        let out = encode(&spec, Some(level), &samples, 4096);
        let block = if level <= 2 { 1152 } else { 4096 };
        check_stream(&spec, &samples, &out, block);
        out.packets.iter().map(|p| p.data.len()).sum::<usize>()
    };
    let sizes: Vec<usize> = (0..=8).map(size).collect();
    assert!(sizes[8] <= sizes[0], "{sizes:?}");
    assert!(sizes[5] < samples.len() * 2, "{sizes:?}");
    for w in sizes.windows(2) {
        // Equal or better, with slack for the odd level that trades block size for speed.
        assert!(w[1] <= w[0] + w[0] / 50, "{sizes:?}");
    }
}

#[test]
fn silence_is_a_tiny_constant_stream() {
    let spec = spec(44100, SampleFormat::S16, 2);
    let samples = vec![0i32; 2 * 44100];
    let out = encode(&spec, None, &samples, 4096);
    check_stream(&spec, &samples, &out, 4096);
    let total: usize = out.packets.iter().map(|p| p.data.len()).sum();
    // Raw would be 176 400 bytes; 11 frames of header plus two constant subframes.
    assert!(total < 11 * 16, "{total}");
}

/// Found by the `flac_roundtrip` fuzz target: these 312 mono 24-bit samples (raw little-endian
/// PCM) came out of levels 7 and 8 as one 33 MB frame, a Rice parameter above 14 written as if
/// it fitted. STREAMINFO's 24-bit frame sizes cannot describe such a frame.
#[test]
fn hostile_24_bit_samples_never_code_larger_than_raw_samples() {
    let pcm = include_bytes!("../../../tests/fixtures/rice-param-overflow-24bit.pcm");
    let samples: Vec<i32> = pcm
        .chunks_exact(3)
        .map(|c| i32::from_le_bytes([c[0], c[1], c[2], 0]) << 8 >> 8)
        .collect();
    assert_eq!(samples.len(), 312);
    let spec = spec(11025, SampleFormat::S24, 1);
    for level in 0..=8 {
        let out = encode(&spec, Some(level), &samples, 100_000);
        for p in &out.packets {
            // Header, one verbatim subframe and the CRC-16, with slack for the header.
            assert!(
                p.data.len() <= 16 + 1 + samples.len() * 3 + 2,
                "level {level}: {} bytes",
                p.data.len()
            );
        }
        check_stream(&spec, &samples, &out, if level <= 2 { 1152 } else { 4096 });
    }
}
