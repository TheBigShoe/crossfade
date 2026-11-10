//! What each coding tool buys, observed through the public encoder and checked by decoding.
mod common;

use common::*;
use crossfade_flac::EncoderConfig;

fn cfg(channels: u8, bits: u8, level: u8) -> EncoderConfig {
    EncoderConfig {
        sample_rate: 44_100,
        channels,
        bits_per_sample: bits,
        level,
    }
}

fn frame_len(e: &Encoded, i: usize) -> usize {
    e.frames[i].1 - e.frames[i].0
}

#[test]
fn zero_low_bits_are_not_coded() {
    // 13 real bits in a 16-bit stream: verbatim samples would cost 16.
    let samples: Vec<i32> = noise(13, 4096 * 2, 5).iter().map(|&v| v << 3).collect();
    let e = round_trip(cfg(2, 16, 5), &samples, 0);
    assert!(
        frame_len(&e, 0) <= 4096 * 2 * 13 / 8 + 40,
        "{}",
        frame_len(&e, 0)
    );
}

#[test]
fn negative_values_with_zero_low_bits_round_trip() {
    let samples: Vec<i32> = noise(10, 1152 * 2, 6).iter().map(|&v| v * -16).collect();
    round_trip(cfg(2, 16, 0), &samples, 0);
}

#[test]
fn a_ramp_is_coded_with_a_first_order_predictor() {
    let samples: Vec<i32> = (0..1152).map(|i| i * 7 - 3000).collect();
    let e = round_trip(cfg(1, 16, 0), &samples, 0);
    assert!(frame_len(&e, 0) <= 40, "{}", frame_len(&e, 0));
}

#[test]
fn a_cubic_is_coded_with_a_third_order_predictor() {
    let samples: Vec<i32> = (0..1152i64).map(|i| (i * i * i) as i32).collect();
    let e = round_trip(cfg(1, 32, 0), &samples, 0);
    assert!(frame_len(&e, 0) <= 40, "{}", frame_len(&e, 0));
}

#[test]
fn level_zero_codes_channels_independently() {
    let samples = noise(16, 1152 * 2, 9);
    let e = round_trip(cfg(2, 16, 0), &samples, 0);
    assert!(matches!(
        assignment(&e, 0),
        crossfade_flac::frame::ChannelAssignment::Independent(2)
    ));
}

#[test]
fn a_noisy_tone_beats_raw_clearly() {
    let samples: Vec<i32> = (0..4096 * 2)
        .map(|i| ((i / 2) as f64 * 0.05).sin().mul_add(20000.0, 0.0) as i32 + (i as i32 % 7))
        .collect();
    let e = round_trip(cfg(2, 16, 0), &samples, 0);
    assert!(
        frame_len(&e, 0) < 1152 * 2 * 2 * 3 / 4,
        "{}",
        frame_len(&e, 0)
    );
}
