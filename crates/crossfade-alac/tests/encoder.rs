//! The encoder against the decoder: ALAC is lossless, so every sample comes back, at every
//! depth and channel count, and audio that can be predicted comes out smaller.

use crossfade_alac::{Config, Decoder, Encoder};
use proptest::prelude::*;

/// Deterministic noise in `-(1 << (bits - 1))..(1 << (bits - 1))`.
fn noise(n: usize, bits: u32, seed: u32) -> Vec<i32> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s as i32) >> (32 - bits)
        })
        .collect()
}

/// A few partials at half scale, the channels a little different.
fn music(frames: usize, channels: usize, bits: u32) -> Vec<i32> {
    let full = f64::from(1u32 << (bits - 1)) * 0.5;
    (0..frames)
        .flat_map(|i| {
            (0..channels).map(move |c| {
                let t = i as f64 / 44_100.0;
                let v = 0.6 * (std::f64::consts::TAU * 220.0 * t).sin()
                    + 0.3 * (std::f64::consts::TAU * (880.0 + 3.0 * c as f64) * t).sin()
                    + 0.1 * (std::f64::consts::TAU * 3520.0 * t + c as f64).sin();
                (v * full) as i32
            })
        })
        .collect()
}

/// Encodes `pcm` packet by packet, decodes each packet, and returns the bytes written.
fn round_trip(config: Config, pcm: &[i32]) -> usize {
    let channels = usize::from(config.channels);
    let mut enc = Encoder::new(config).unwrap();
    let mut dec = Decoder::new(config).unwrap();
    let mut bytes = 0;
    let (mut packet, mut out) = (vec![], vec![]);
    for chunk in pcm.chunks(config.frame_length as usize * channels) {
        packet.clear();
        enc.encode(chunk, &mut packet).unwrap();
        let frames = dec.decode(&packet, &mut out).unwrap();
        assert_eq!(frames, chunk.len() / channels);
        assert!(
            out == chunk,
            "{config:?}: the packet decodes to other samples"
        );
        bytes += packet.len();
    }
    bytes
}

#[test]
fn every_depth_and_channel_count_comes_back_sample_for_sample() {
    for bits in [16u8, 20, 24, 32] {
        for channels in 1..=8u8 {
            let config = Config::new(44_100, channels, bits).unwrap();
            assert_eq!(
                (config.frame_length, config.channels, config.bit_depth),
                (4096, channels, bits)
            );
            assert_eq!(Config::parse(&config.write()).unwrap(), config);
            let c = usize::from(channels);
            // Two packets and a short third.
            round_trip(config, &music(4096 * 2 + 999, c, u32::from(bits)));
            round_trip(
                config,
                &noise((4096 + 17) * c, u32::from(bits), 7 + u32::from(bits)),
            );
            round_trip(config, &vec![0; 5000 * c]);
            // Full scale both ways, alternating: the worst a predictor can meet.
            let top = ((1i64 << (bits - 1)) - 1) as i32;
            let harsh: Vec<i32> = (0..3000 * c)
                .map(|i| if i % 3 == 0 { top } else { -top - 1 })
                .collect();
            round_trip(config, &harsh);
        }
    }
}

#[test]
fn music_comes_out_smaller_and_noise_no_larger_than_raw() {
    for (bits, limit) in [(16u8, 0.55), (24, 0.70)] {
        let config = Config::new(44_100, 2, bits).unwrap();
        let frames = 4096 * 8;
        let raw = frames * 2 * usize::from(bits) / 8;
        let bytes = round_trip(config, &music(frames, 2, u32::from(bits)));
        assert!(
            (bytes as f64) < limit * raw as f64,
            "{bits}-bit music: {bytes} of {raw}"
        );
        // Noise cannot be predicted: each packet falls back to raw samples and a few bytes.
        let bytes = round_trip(config, &noise(frames * 2, u32::from(bits), 99));
        assert!(bytes <= raw + 8 * 8, "{bits}-bit noise: {bytes} of {raw}");
    }
    // Silence costs next to nothing.
    let config = Config::new(48_000, 2, 16).unwrap();
    assert!(round_trip(config, &vec![0; 4096 * 2 * 4]) < 200);
    // Two channels alike cost little more than one.
    let mono = music(4096 * 4, 1, 16);
    let twice: Vec<i32> = mono.iter().flat_map(|&v| [v, v]).collect();
    let one = round_trip(Config::new(44_100, 1, 16).unwrap(), &mono);
    let two = round_trip(config, &twice);
    assert!(
        two * 10 < one * 12,
        "{two} for two alike against {one} for one"
    );
}

#[test]
fn bad_arguments_are_refused_and_append_nothing() {
    assert!(Config::new(44_100, 0, 16).is_err());
    assert!(Config::new(44_100, 9, 16).is_err());
    assert!(Config::new(44_100, 2, 8).is_err());
    let config = Config::new(44_100, 2, 16).unwrap();
    let mut enc = Encoder::new(config).unwrap();
    let mut out = vec![1, 2];
    // Not whole frames, more than a packet's worth, nothing at all, a sample too wide.
    assert!(enc.encode(&[0; 3], &mut out).is_err());
    assert!(enc.encode(&vec![0; 4097 * 2], &mut out).is_err());
    assert!(enc.encode(&[], &mut out).is_err());
    assert!(enc.encode(&[40_000, 0], &mut out).is_err());
    assert_eq!(out, [1, 2]);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn any_samples_round_trip(
        bits in prop::sample::select(vec![16u8, 20, 24, 32]),
        channels in 1u8..=8,
        frames in 1usize..300,
        seed in any::<u32>(),
        smooth in any::<bool>(),
    ) {
        let config = Config { frame_length: 128, ..Config::new(48_000, channels, bits).unwrap() };
        let c = usize::from(channels);
        let pcm = if smooth {
            music(frames, c, u32::from(bits))
        } else {
            noise(frames * c, u32::from(bits), seed)
        };
        round_trip(config, &pcm);
    }
}
