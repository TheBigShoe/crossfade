//! `Encoder::write` and `flush` take any chunk size and must give the bytes that exact-block
//! `encode_block` calls give.
mod common;

use common::*;
use crossfade_flac::{Encoder, EncoderConfig, Error};

fn cfg(channels: u8, bits: u8, level: u8) -> EncoderConfig {
    EncoderConfig {
        sample_rate: 48_000,
        channels,
        bits_per_sample: bits,
        level,
        threads: 1,
    }
}

fn chunked(
    config: EncoderConfig,
    samples: &[i32],
    seed: u64,
) -> (Vec<u8>, crossfade_flac::streaminfo::StreamInfoBlock) {
    let mut enc = Encoder::new(config).unwrap();
    let ch = usize::from(config.channels);
    let mut rng = Rng(seed | 1);
    let mut out = Vec::new();
    let mut at = 0;
    while at < samples.len() {
        // Sizes from nothing up to a couple of blocks, in whole sample frames.
        let frames = (rng.next() % (2 * enc.block_size() as u64 + 3)) as usize;
        let end = (at + frames * ch).min(samples.len());
        enc.write(&samples[at..end], &mut out).unwrap();
        at = end;
    }
    enc.flush(&mut out).unwrap();
    enc.flush(&mut out).unwrap(); // nothing left: no frame
    let info = enc.finish();
    (out, info)
}

#[test]
fn any_chunking_equals_exact_blocks() {
    for (channels, bits, level) in [(1, 16, 5), (2, 16, 0), (2, 24, 5), (3, 8, 8), (2, 32, 3)] {
        let config = cfg(channels, bits, level);
        let ch = usize::from(channels);
        let block = Encoder::new(config).unwrap().block_size();
        for frames in [1, block - 1, block, block + 1, 3 * block, 3 * block + 17] {
            let samples = noise(bits.min(12), frames * ch, 5)
                .iter()
                .enumerate()
                .map(|(i, &v)| if i % 7 < 3 { 0 } else { v })
                .collect::<Vec<_>>();
            let exact = encode(config, &samples, 0);
            for seed in 1..4 {
                let (bytes, info) = chunked(config, &samples, seed);
                assert_eq!(bytes, exact.bytes, "{config:?} frames {frames} seed {seed}");
                assert_eq!(info, exact.info);
            }
        }
    }
}

#[test]
fn a_full_block_is_emitted_without_waiting_for_flush() {
    let config = cfg(2, 16, 5);
    let mut enc = Encoder::new(config).unwrap();
    let mut out = Vec::new();
    enc.write(&noise(16, 4096 * 2 - 2, 1), &mut out).unwrap();
    assert!(out.is_empty());
    enc.write(&noise(16, 2, 2), &mut out).unwrap();
    assert!(!out.is_empty());
}

#[test]
fn invalid_writes_change_nothing() {
    let config = cfg(2, 8, 5);
    let mut enc = Encoder::new(config).unwrap();
    let mut out = Vec::new();
    enc.write(&[1, 2, 3, 4], &mut out).unwrap();
    let m = |r: Result<(), Error>| assert!(matches!(r, Err(Error::Malformed(_))), "{r:?}");
    m(enc.write(&[1, 2, 3], &mut out)); // not whole sample frames
    m(enc.write(&[1, 200], &mut out)); // beyond 8 bits
    m(enc.encode_block(&[1, 2], &mut out)); // samples are pending
    enc.flush(&mut out).unwrap();
    assert!(!out.is_empty());
    let reference = round_trip(config, &[1, 2, 3, 4], 0);
    assert_eq!(out, reference.bytes);
    m(enc.write(&[1, 2], &mut out)); // a short block ended the stream
}

#[test]
fn frame_ends_mark_every_frame_in_chunked_output() {
    let config = cfg(2, 16, 5);
    let samples = noise(14, (4096 * 2 + 100) * 2, 3);
    let exact = encode(config, &samples, 0);
    let want: Vec<usize> = exact.frames.iter().map(|f| f.1).collect();
    for step in [1usize, 700, 4096, 9000] {
        let mut enc = Encoder::new(config).unwrap();
        let (mut out, mut ends) = (vec![0xAA], Vec::new());
        for chunk in samples.chunks(step * 2) {
            enc.write_ends(chunk, &mut out, &mut ends).unwrap();
        }
        enc.flush_ends(&mut out, &mut ends).unwrap();
        // Offsets are into `out`, which already held one byte.
        let got: Vec<usize> = ends.iter().map(|e| e - 1).collect();
        assert_eq!(got, want, "step {step}");
        assert_eq!(&out[1..], &exact.bytes[..]);
    }
}
