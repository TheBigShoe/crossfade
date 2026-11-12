//! The decoder against Apple's (`afconvert -f WAVE -d LEF32`, macOS) on the streams Apple's
//! encoder made of the deterministic signals in `crossfade-format/tests/fixtures/adts`: a few
//! hundred reference frames per stream (`tests/fixtures/apple/*.f32`), held to the ISO/IEC
//! 13818-4 full-accuracy bounds, and a digest of our whole output so any change is noticed.
//! `CROSSFADE_BLESS=1` rewrites `digests.txt` after a deliberate change.
#![cfg(feature = "decoder")]

use crossfade_aac::Decoder;
use crossfade_aac::adts::Header;
use crossfade_aac::asc::AudioSpecificConfig;
use std::fmt::Write as _;

const STREAMS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../crossfade-format/tests/fixtures/adts"
);
const APPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/apple");

/// RMS bound: 2^-15 / sqrt(12) of full scale.
const MAX_RMS: f64 = 8.8e-6;
/// Largest single difference: 2^-14.
const MAX_DIFF: f64 = 1.0 / 16384.0;

const NAMES: [&str; 9] = [
    "tone_44k_stereo",
    "sweep_48k_mono",
    "clicks_44k_stereo",
    "noise_44k_stereo",
    "silence_44k_stereo",
    "tone_16k_mono",
    "tone_8k_mono",
    "mix_44k_stereo_64k",
    "tones_48k_51",
];

/// Raw data blocks of an ADTS stream and the decoder its first header calls for.
fn open(name: &str) -> (Decoder, Vec<Vec<u8>>) {
    let bytes = std::fs::read(format!("{STREAMS}/{name}.aac")).unwrap();
    let first = Header::parse(&bytes).unwrap();
    let mut blocks = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let h = Header::parse(&bytes[at..]).unwrap();
        blocks.push(bytes[at + h.header_len()..at + usize::from(h.frame_length)].to_vec());
        at += usize::from(h.frame_length);
    }
    let config = AudioSpecificConfig::parse(&first.audio_specific_config()).unwrap();
    (Decoder::new(&config).unwrap(), blocks)
}

fn decode_all(name: &str) -> (usize, Vec<f32>) {
    let (mut d, blocks) = open(name);
    let mut all = Vec::new();
    let mut out = Vec::new();
    for b in &blocks {
        let frames = d.decode(b, &mut out).unwrap();
        assert_eq!(frames, 1024);
        assert_eq!(out.len(), frames * d.channels());
        all.extend_from_slice(&out);
    }
    (d.channels(), all)
}

/// (start frame, interleaved samples) windows of Apple's decode.
fn reference(name: &str, channels: usize) -> Vec<(usize, Vec<f32>)> {
    let bytes = std::fs::read(format!("{APPLE}/{name}.f32")).unwrap();
    let mut out = Vec::new();
    let mut at = 0;
    let word = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    while at < bytes.len() {
        let (start, frames) = (word(at) as usize, word(at + 4) as usize);
        at += 8;
        let samples = (0..frames * channels)
            .map(|i| f32::from_bits(word(at + 4 * i)))
            .collect();
        at += 4 * frames * channels;
        out.push((start, samples));
    }
    out
}

#[test]
fn every_stream_matches_apples_decode_to_full_accuracy() {
    let mut report = String::new();
    for name in NAMES {
        let (channels, ours) = decode_all(name);
        let (mut sum, mut n, mut max) = (0.0f64, 0usize, 0.0f64);
        for (start, want) in reference(name, channels) {
            let got = &ours[start * channels..start * channels + want.len()];
            for (&g, &w) in got.iter().zip(&want) {
                let d = f64::from(g) - f64::from(w);
                sum += d * d;
                n += 1;
                max = max.max(d.abs());
            }
        }
        let rms = (sum / n as f64).sqrt();
        writeln!(report, "{name}: rms {rms:.3e}, max {max:.3e}").unwrap();
        assert!(rms < MAX_RMS && max < MAX_DIFF, "{report}");
    }
    println!("{report}");
}

fn fnv1a(samples: &[f32]) -> u64 {
    samples.iter().fold(0xcbf2_9ce4_8422_2325, |h, s| {
        s.to_bits()
            .to_le_bytes()
            .iter()
            .fold(h, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
    })
}

#[test]
fn the_output_is_frozen_bit_for_bit() {
    let mut text = String::new();
    for name in NAMES.iter().chain(&["mix_44k_stereo_he"]) {
        let (_, ours) = decode_all(name);
        writeln!(text, "{name} {:016x}", fnv1a(&ours)).unwrap();
    }
    let path = format!("{APPLE}/digests.txt");
    if std::env::var_os("CROSSFADE_BLESS").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), text);
}

#[test]
fn he_aac_plays_its_core_and_says_sbr_was_there() {
    let (mut d, blocks) = open("mix_44k_stereo_he");
    assert_eq!((d.sample_rate(), d.channels()), (22050, 2));
    let mut out = Vec::new();
    for b in &blocks {
        assert_eq!(d.decode(b, &mut out), Ok(1024));
    }
    assert!(d.sbr_present());
    // The core carries the mix's low band: not silence.
    assert!(out.iter().any(|s| s.abs() > 0.01));
}

#[test]
fn decoding_after_a_reset_is_decoding_afresh() {
    let (mut d, blocks) = open("clicks_44k_stereo");
    let mut first = Vec::new();
    let mut out = Vec::new();
    for b in &blocks[..10] {
        d.decode(b, &mut out).unwrap();
        first.extend_from_slice(&out);
    }
    d.reset();
    let mut again = Vec::new();
    for b in &blocks[..10] {
        d.decode(b, &mut out).unwrap();
        again.extend_from_slice(&out);
    }
    assert_eq!(first, again);
}

#[test]
fn blocks_back_to_back_decode_as_they_do_one_at_a_time() {
    let (mut d, blocks) = open("tone_44k_stereo");
    let mut one = Vec::new();
    let mut out = Vec::new();
    for b in &blocks[..12] {
        d.decode(b, &mut out).unwrap();
        one.extend_from_slice(&out);
    }
    d.reset();
    let mut joined = Vec::new();
    for group in blocks[..12].chunks(4) {
        let data = group.concat();
        assert_eq!(d.decode_blocks(&data, group.len(), &mut out), Ok(4096));
        joined.extend_from_slice(&out);
    }
    assert_eq!(one, joined);
}
