//! Unit tests that reach inside the encoder: forced stereo modes.

use super::*;
use crate::Decoder;
use crate::frame::ChannelAssignment::{LeftSide, MidSide, SideRight};

fn pseudo(bits: u8, count: usize, seed: u64) -> Vec<i32> {
    let mut s = seed | 1;
    (0..count)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s as i64 >> (64 - u32::from(bits))) as i32
        })
        .collect()
}

/// Encodes one block with the stereo mode forced and decodes it again.
fn forced(bits: u8, level: u8, mode: ChannelAssignment, samples: &[i32]) {
    let mut enc = Encoder::new(EncoderConfig {
        sample_rate: 44_100,
        channels: 2,
        bits_per_sample: bits,
        level,
    })
    .unwrap();
    enc.force_stereo(mode);
    let mut out = Vec::new();
    enc.encode_block(samples, &mut out).unwrap();
    let info = enc.finish();
    let header = frame::parse_header(&out).unwrap();
    assert_eq!(header.channels, mode, "bits {bits} level {level}");
    let mut dec = Decoder::new(&info).unwrap();
    let mut back = Vec::new();
    dec.decode_frame(&out).unwrap().interleave_i32(0, &mut back);
    assert_eq!(back, samples, "bits {bits} level {level} {mode:?}");
    assert_eq!(dec.finish().unwrap(), crate::Verification::Md5Matched);
}

#[test]
fn every_stereo_mode_round_trips_at_every_depth() {
    for mode in [LeftSide, SideRight, MidSide] {
        for bits in [4u8, 8, 16, 24, 31, 32] {
            let lo = if bits == 32 { i32::MIN } else { -(1 << (bits - 1)) };
            let hi = if bits == 32 { i32::MAX } else { (1 << (bits - 1)) - 1 };
            let mut signals = vec![pseudo(bits, 2 * 300, 11)];
            // Extremes make the side channel as wide as it gets (33 bits at 32-bit depth).
            signals.push((0..200).map(|i| if i % 4 < 2 { lo } else { hi }).collect());
            signals.push([lo, hi].repeat(50));
            signals.push([hi, lo].repeat(50));
            signals.push([lo, lo].repeat(40));
            signals.push([hi, lo, lo, hi, hi, hi, lo, lo].to_vec());
            signals.push([1, 0, 0, 1, -1, 0, 0, -1].repeat(9));
            for s in &signals {
                for level in [0, 5, 8] {
                    forced(bits, level, mode, s);
                }
            }
        }
    }
}
