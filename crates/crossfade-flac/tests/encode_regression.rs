//! Inputs that once broke an encoder, compression sanity, and golden hashes of the output.
mod common;

use common::*;
use crossfade_flac::EncoderConfig;
use crossfade_flac::md5::Md5;

fn cfg(rate: u32, channels: u8, bits: u8, level: u8) -> EncoderConfig {
    EncoderConfig {
        sample_rate: rate,
        channels,
        bits_per_sample: bits,
        level,
    }
}

/// Mono 24-bit samples (raw little-endian) that made the previous encoder write frames far
/// larger than the audio: its Rice parameter outgrew the 4-bit field.
#[test]
fn rice_parameter_overflow_inputs_stay_within_the_raw_bound() {
    let files: [&[u8]; 2] = [
        include_bytes!("fixtures/rice-param-overflow-24bit.pcm"),
        include_bytes!("fixtures/rice-param-overflow-24bit-64.pcm"),
    ];
    for pcm in files {
        let samples: Vec<i32> = pcm
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], 0]) << 8 >> 8)
            .collect();
        for level in 0..=8 {
            let e = round_trip(cfg(11_025, 1, 24, level), &samples, 0);
            assert_frames_within_bound(&e, samples.len());
        }
    }
}

/// A resonant tone with noise, from integer arithmetic only so it is identical everywhere.
/// The second channel follows the first.
fn resonator(frames: usize, channels: usize, bits: u8, noise: u64, seed: u64) -> Vec<i32> {
    let mut rng = Rng(seed | 1);
    let (lo, hi) = (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1);
    let mut state = [(0i64, 0i64); 8];
    let mut out = Vec::with_capacity(frames * channels);
    let mut first = 0i64;
    for _ in 0..frames {
        for (c, s) in state.iter_mut().enumerate().take(channels) {
            let kick = (rng.next() % (2 * noise + 1)) as i64 - noise as i64;
            let y = ((s.0 * 31_130) >> 14) - ((s.1 * 16_302) >> 14) + kick;
            *s = (y, s.0);
            let v = if c == 0 {
                first = y;
                y
            } else {
                first * 3 / 4 + (rng.next() % 9) as i64 - 4
            };
            let scaled = if bits >= 16 {
                v << (bits - 16)
            } else {
                v >> (16 - bits)
            };
            out.push(scaled.clamp(lo, hi) as i32);
        }
    }
    out
}

#[test]
fn each_level_is_no_larger_than_the_previous_by_more_than_one_percent() {
    let samples = resonator(4096 * 6 + 100, 2, 16, 2000, 1);
    let sizes: Vec<usize> = (0..=8)
        .map(|level| {
            round_trip(cfg(44_100, 2, 16, level), &samples, 0)
                .bytes
                .len()
        })
        .collect();
    for level in 1..=8 {
        assert!(
            sizes[level] as f64 <= sizes[level - 1] as f64 * 1.01,
            "level {level}: {} vs {} at level {}",
            sizes[level],
            sizes[level - 1],
            level - 1
        );
    }
    let quiet = resonator(4096 * 3, 2, 16, 40, 2);
    let e = round_trip(cfg(44_100, 2, 16, 5), &quiet, 0);
    let raw = quiet.len() * 2;
    assert!(
        e.bytes.len() * 10 < raw * 5,
        "level 5: {} of {raw}",
        e.bytes.len()
    );
}

fn hex(bytes: &[u8]) -> String {
    let mut md5 = Md5::new();
    md5.update(bytes);
    md5.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// A change in the output must be deliberate: these are the MD5 of the whole encoded stream
/// (frames only) for fixed signals. Integer-only input; the encoder's floating point uses
/// only exactly-defined operations, so every platform must reproduce them.
#[test]
fn output_bytes_are_pinned() {
    let stereo16 = resonator(4096 * 3 + 333, 2, 16, 2000, 7);
    let mono24 = resonator(4096 + 500, 1, 24, 2000, 8);
    let wide32 = resonator(4096 + 71, 2, 32, 2000, 9);
    let multi = resonator(1152 * 2 + 5, 6, 16, 2000, 10);
    let mut got = Vec::new();
    for level in 0..=8 {
        got.push((
            format!("stereo16 L{level}"),
            encode(cfg(44_100, 2, 16, level), &stereo16, 0),
        ));
    }
    for level in [0, 5, 8] {
        got.push((
            format!("mono24 L{level}"),
            encode(cfg(96_000, 1, 24, level), &mono24, 0),
        ));
        got.push((
            format!("wide32 L{level}"),
            encode(cfg(44_100, 2, 32, level), &wide32, 0),
        ));
        got.push((
            format!("multi6 L{level}"),
            encode(cfg(48_000, 6, 16, level), &multi, 0),
        ));
    }
    let table: Vec<(String, String)> = got
        .iter()
        .map(|(name, e)| {
            let mut all = e.bytes.clone();
            all.extend_from_slice(&e.info.write());
            (name.clone(), hex(&all))
        })
        .collect();
    let want = GOLDEN;
    let actual: Vec<String> = table.iter().map(|(n, h)| format!("{n} {h}")).collect();
    assert_eq!(actual, want, "\nactual:\n{}", actual.join("\n"));
}

const GOLDEN: [&str; 18] = [
    "stereo16 L0 ca22fee9d1500879392405a6ce1e1fa0",
    "stereo16 L1 2900316735a549e74a4afd44e7b044b2",
    "stereo16 L2 2900316735a549e74a4afd44e7b044b2",
    "stereo16 L3 a7a9d89ba9a369dd839951e0b7303b34",
    "stereo16 L4 e82896580a6a3c19d495ae7fc2c93e54",
    "stereo16 L5 fd90057f1f6797e2843a7d7405136705",
    "stereo16 L6 412d3cd5b0583fdbf31a4c800d6e9fb4",
    "stereo16 L7 a48ba54aea4f29c90af662d0ba69f14a",
    "stereo16 L8 f0f4c1a0cb05a88fb4f35597971dc7a6",
    "mono24 L0 0c6626228bf0c872c369d5b6ebdb4479",
    "wide32 L0 b1ebbb4f08802356d60e327bcf88609c",
    "multi6 L0 d48a6d31f3d690b0666ef7e8398ba2f0",
    "mono24 L5 68730c97280fbc5056c619a97ddc523d",
    "wide32 L5 2cb49fd46dc9c0c8320a4dddaceb233e",
    "multi6 L5 9f166b4741f01822764eee9f651ac300",
    "mono24 L8 fd89363dbb8064be79e114fdb318a84f",
    "wide32 L8 ba029bf77045c47ad33ae55abaccf454",
    "multi6 L8 e47a341d49928db9700c78ec846a8500",
];
