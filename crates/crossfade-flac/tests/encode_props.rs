//! Property tests: whatever the format, content and chunking, the encoder's output decodes
//! bit-exactly, checks out against its own MD5 and never exceeds a raw frame.
mod common;

use common::*;
use crossfade_flac::{Encoder, EncoderConfig};
use proptest::prelude::*;

const RATES: [u32; 8] = [
    1, 8_000, 44_100, 48_000, 96_000, 192_000, 655_350, 1_048_575,
];

fn config(rate: usize, channels: u8, bits: u8, level: u8) -> EncoderConfig {
    EncoderConfig {
        sample_rate: RATES[rate % RATES.len()],
        channels,
        bits_per_sample: bits,
        level,
    }
}

/// How many sample frames to encode: tiny, ordinary, or around the block boundaries.
fn frames_for(block: usize, pick: u8, frac: u16) -> usize {
    let f = usize::from(frac);
    match pick % 5 {
        0 => 1 + f % 40,
        1 => 41 + f % 600,
        2 => block - 1 + f % 3,
        3 => block + 1 + f % 90,
        _ => 2 * block + f % 50,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn encode_then_decode_is_bit_exact_and_bounded(
        bits in 4u8..=32,
        channels in 1u8..=8,
        rate in 0usize..8,
        level in 0u8..=8,
        kind in 0u8..KINDS,
        pick in any::<u8>(),
        frac in any::<u16>(),
        seed in any::<u64>(),
    ) {
        let config = config(rate, channels, bits, level);
        let block = Encoder::new(config).unwrap().block_size();
        // Keep a case cheap: many channels get shorter streams.
        let frames = frames_for(block, pick, frac).min(block * 2 + 40);
        let frames = if channels > 4 { frames.min(block + 40) } else { frames };
        let samples = signal(kind, bits, frames * usize::from(channels), usize::from(channels), seed);
        let e = round_trip(config, &samples, 0);
        assert_frames_within_bound(&e, frames);
        prop_assert_eq!(e.frames.len(), frames.div_ceil(block));
        prop_assert_eq!(e.info.total_samples, frames as u64);
        // The same input and settings give the same bytes from a fresh encoder.
        prop_assert_eq!(&encode(config, &samples, 0).bytes, &e.bytes);
    }

    #[test]
    fn chunked_writes_equal_exact_blocks(
        bits in 4u8..=32,
        channels in 1u8..=4,
        level in 0u8..=8,
        kind in 0u8..KINDS,
        pick in any::<u8>(),
        frac in any::<u16>(),
        seed in any::<u64>(),
    ) {
        let config = config(2, channels, bits, level);
        let ch = usize::from(channels);
        let block = Encoder::new(config).unwrap().block_size();
        let frames = frames_for(block, pick, frac).min(block * 2 + 40);
        let samples = signal(kind, bits, frames * ch, ch, seed);
        let exact = encode(config, &samples, 0);

        let mut enc = Encoder::new(config).unwrap();
        let mut rng = Rng(seed ^ 0xdead_beef);
        let mut out = Vec::new();
        let mut at = 0;
        while at < samples.len() {
            let n = (rng.next() % (block as u64 + 5)) as usize * ch;
            let end = (at + n).min(samples.len());
            enc.write(&samples[at..end], &mut out).unwrap();
            at = end;
        }
        enc.flush(&mut out).unwrap();
        prop_assert_eq!(enc.finish(), exact.info);
        prop_assert_eq!(out, exact.bytes);
    }
}
