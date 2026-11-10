//! The encoder's public contract: configuration limits, block rules and a first round trip.
mod common;

use common::*;
use crossfade_flac::{Encoder, EncoderConfig, Error};

fn cfg(rate: u32, channels: u8, bits: u8, level: u8) -> EncoderConfig {
    EncoderConfig {
        sample_rate: rate,
        channels,
        bits_per_sample: bits,
        level,
    }
}

#[test]
fn new_sets_the_default_level() {
    assert_eq!(EncoderConfig::new(44_100, 2, 16).level, 5);
}

#[test]
fn out_of_range_fields_are_unsupported() {
    for bad in [
        cfg(0, 2, 16, 5),
        cfg(1_048_576, 2, 16, 5),
        cfg(44_100, 0, 16, 5),
        cfg(44_100, 9, 16, 5),
        cfg(44_100, 2, 3, 5),
        cfg(44_100, 2, 33, 5),
        cfg(44_100, 2, 16, 9),
    ] {
        assert!(
            matches!(Encoder::new(bad), Err(Error::Unsupported(_))),
            "{bad:?}"
        );
    }
    for ok in [cfg(1, 1, 4, 0), cfg(1_048_575, 8, 32, 8)] {
        assert!(Encoder::new(ok).is_ok(), "{ok:?}");
    }
}

#[test]
fn block_size_follows_the_level() {
    for level in 0..=8 {
        let enc = Encoder::new(cfg(44_100, 2, 16, level)).unwrap();
        assert_eq!(enc.block_size(), if level <= 2 { 1152 } else { 4096 });
    }
}

#[test]
fn silence_becomes_a_few_bytes_per_frame() {
    for level in 0..=8 {
        let config = cfg(44_100, 2, 16, level);
        let enc = Encoder::new(config).unwrap();
        let samples = vec![0; enc.block_size() * 2 * 3 + 10];
        let e = round_trip(config, &samples, 0);
        assert_eq!(e.frames.len(), 4);
        for &(s, t) in &e.frames {
            assert!(t - s <= 16, "level {level}: {} bytes", t - s);
        }
    }
}

#[test]
fn noise_round_trips_within_the_raw_bound() {
    for bits in [4u8, 8, 16, 24, 32] {
        let config = cfg(48_000, 2, bits, 5);
        let samples = noise(bits, 4096 * 2 * 2 + 14, 7);
        let e = round_trip(config, &samples, 0);
        assert_within_bound(&e);
    }
}

#[test]
fn stream_info_before_and_after_finish() {
    let config = cfg(44_100, 2, 16, 5);
    let mut enc = Encoder::new(config).unwrap();
    let before = enc.stream_info();
    assert_eq!((before.total_samples, before.md5), (0, [0; 16]));
    assert_eq!(
        (before.sample_rate, before.channels, before.bits_per_sample),
        (44_100, 2, 16)
    );
    let mut out = Vec::new();
    enc.encode_block(&noise(16, 4096 * 2, 1), &mut out).unwrap();
    enc.encode_block(&noise(16, 10 * 2, 2), &mut out).unwrap();
    let info = enc.finish();
    assert_eq!(info.total_samples, 4106);
    assert_eq!((info.min_block_size, info.max_block_size), (4096, 4096));
    assert_ne!(info.md5, [0; 16]);
    assert!(info.min_frame_size > 0 && info.min_frame_size <= info.max_frame_size);
    assert_eq!(enc.finish(), info, "finish is idempotent");
}

#[test]
fn a_single_short_block_sets_both_block_sizes() {
    let config = cfg(44_100, 1, 16, 5);
    let e = round_trip(config, &noise(16, 100, 3), 0);
    assert_eq!((e.info.min_block_size, e.info.max_block_size), (100, 100));
}

#[test]
fn bad_blocks_are_malformed() {
    let config = cfg(44_100, 2, 8, 0);
    let mut enc = Encoder::new(config).unwrap();
    let mut out = Vec::new();
    let m = |r: Result<(), Error>| assert!(matches!(r, Err(Error::Malformed(_))), "{r:?}");
    m(enc.encode_block(&[], &mut out));
    m(enc.encode_block(&[0, 0, 0], &mut out)); // not a whole number of sample frames
    m(enc.encode_block(&[128, 0], &mut out)); // 128 needs 9 bits
    m(enc.encode_block(&[0, -129], &mut out));
    m(enc.encode_block(&vec![0; 1153 * 2], &mut out)); // longer than a block
    assert!(out.is_empty(), "failed calls write nothing");
    enc.encode_block(&[1, 2, 3, 4], &mut out).unwrap(); // short: the last block
    m(enc.encode_block(&[1, 2], &mut out));
}

#[test]
fn a_single_block_under_sixteen_samples_keeps_a_parsable_streaminfo() {
    // STREAMINFO cannot hold a block size below 16 (its own parser refuses it).
    let config = cfg(8_000, 1, 16, 5);
    let e = round_trip(config, &noise(16, 10, 3), 0);
    assert_eq!((e.info.min_block_size, e.info.max_block_size), (4096, 4096));
    assert!(crossfade_flac::streaminfo::StreamInfoBlock::parse(&e.info.write()).is_ok());
}
