#![no_main]
//! The standalone encoder's property: whatever format and samples the bytes describe, the
//! output decodes to exactly the input, the MD5 in the final STREAMINFO matches, no frame is
//! larger than one made of verbatim subframes, and chunked writes, on one to three threads,
//! give the same bytes and STREAMINFO as exact blocks. Panics by design when any of that fails.

use crossfade_flac::{Decoder, Encoder, EncoderConfig, Verification, frame};
use libfuzzer_sys::fuzz_target;

const RATES: [u32; 8] = [1, 8000, 44100, 48000, 96000, 192000, 655_350, 1_048_575];
/// Keeps one run fast; blocks are at most 4096 samples, so this still spans several.
const MAX_FRAMES: usize = 12_000;

fuzz_target!(|data: &[u8]| {
    let [bits, channels, level, rate, shape, seed, body @ ..] = data else {
        return;
    };
    let bits = 4 + bits % 29;
    let channels = 1 + channels % 8;
    let config = EncoderConfig {
        sample_rate: RATES[usize::from(*rate) % RATES.len()],
        channels,
        bits_per_sample: bits,
        level: level % 9,
        threads: 1,
    };
    let ch = usize::from(channels);
    let width = usize::from(bits).div_ceil(8);
    let (lo, hi) = if bits == 32 {
        (i64::from(i32::MIN), i64::from(i32::MAX))
    } else {
        (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1)
    };

    // Raw sample bytes, or deltas that build smooth signals so the predictors get exercised;
    // optionally with the low bits cleared.
    let mut walk = 0i64;
    let mut samples: Vec<i32> = body
        .chunks_exact(width)
        .take(MAX_FRAMES * ch)
        .map(|c| {
            let mut w = [0u8; 4];
            w[..width].copy_from_slice(c);
            let raw = i64::from(i32::from_le_bytes(w) << (32 - 8 * width) >> (32 - 8 * width));
            let v = match shape % 3 {
                0 => raw.clamp(lo, hi),
                1 => {
                    walk = (walk + (raw >> (8 * width as u32 - 6))).clamp(lo, hi);
                    walk
                }
                _ => (raw.clamp(lo, hi) >> 3) << 3,
            };
            v as i32
        })
        .collect();
    samples.truncate(samples.len() / ch * ch);
    if samples.is_empty() {
        return;
    }
    let frames = samples.len() / ch;

    let mut enc = Encoder::new(config).unwrap();
    let block = enc.block_size();
    let mut bytes = Vec::new();
    let mut ends = Vec::new();
    for chunk in samples.chunks(block * ch) {
        enc.encode_block(chunk, &mut bytes).unwrap();
        ends.push(bytes.len());
    }
    let info = enc.finish();

    let mut dec = Decoder::new(&info).unwrap();
    let mut back = Vec::new();
    let mut tmp = Vec::new();
    let mut start = 0;
    for (n, &end) in ends.iter().enumerate() {
        let data = &bytes[start..end];
        let header = frame::parse_header(data).unwrap();
        assert_eq!(header.number, n as u64);
        let len = block.min(frames - n * block);
        let bound = 16 + 2 + ch * (1 + (len * usize::from(bits)).div_ceil(8)) + 1;
        assert!(data.len() <= bound, "frame {n}: {} > {bound}", data.len());
        dec.decode_frame(data).unwrap().interleave_i32(0, &mut tmp);
        back.extend_from_slice(&tmp);
        start = end;
    }
    assert!(back == samples, "decoded samples differ from the input");
    assert_eq!(dec.finish().unwrap(), Verification::Md5Matched);

    // Arbitrary chunking gives the same stream, whatever the thread count.
    let mut chunked = Encoder::new(EncoderConfig {
        threads: 1 + usize::from(*seed % 3),
        ..config
    })
    .unwrap();
    let mut out = Vec::new();
    let step = (usize::from(*seed) + 1) * ch;
    for chunk in samples.chunks(step) {
        chunked.write(chunk, &mut out).unwrap();
    }
    chunked.flush(&mut out).unwrap();
    assert!(out == bytes, "chunked output differs");
    assert_eq!(chunked.finish(), info);
});
