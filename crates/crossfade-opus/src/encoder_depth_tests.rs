//! The input's depth (`lsb_depth`): what it changes, against libopus's own decisions (its
//! float build at `OPUS_SET_LSB_DEPTH(16)` on the same kinds of signal).

use super::modes_tests::modes;
use super::tests::{config, digest, encode_and_decode, signal};
use super::*;
use crate::MultistreamEncoder;

/// Two seconds of `channels` tones at `amp` (440 Hz, then 110 Hz higher per channel), with
/// white noise at `hiss`.
fn tone_and_hiss(channels: usize, amp: f32, hiss: f32) -> Vec<f32> {
    let tau = 2.0 * std::f32::consts::PI;
    let mut seed = 1u32;
    let mut out = Vec::with_capacity(96_000 * channels);
    for i in 0..96_000 {
        let t = i as f32 / 48_000.0;
        for ch in 0..channels {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = ((seed >> 8) as f32 / (1 << 24) as f32) * 2.0 - 1.0;
            out.push(amp * libm::sinf(tau * (440.0 + 110.0 * ch as f32) * t) + hiss * noise);
        }
    }
    out
}

fn at_depth(lsb_depth: u8, cfg: EncoderConfig) -> EncoderConfig {
    EncoderConfig { lsb_depth, ..cfg }
}

#[test]
fn a_signal_under_the_input_depth_is_digital_silence() {
    // 1e-5 is under a 16-bit step (2^-16) and over a 24-bit one. libopus codes it at
    // 1.2 kb/s (three bytes a packet) at 16 bits, at 45 kb/s at 24.
    let pcm = tone_and_hiss(2, 1e-5, 0.0);
    for (depth, silent) in [(24, false), (16, true)] {
        let cfg = at_depth(depth, config(2, 64_000, true, FrameDuration::Ms20));
        let (packets, _) = encode_and_decode(&cfg, &pcm);
        let tiny = packets.iter().filter(|p| p.len() <= 3).count();
        let want = if silent { packets.len() } else { 0 };
        assert_eq!(tiny, want, "{depth} bits");
    }
    // Every stream of a multistream encoder has the depth.
    let pcm = tone_and_hiss(6, 1e-5, 0.0);
    for (depth, most) in [(24, usize::MAX), (16, 4 * 4)] {
        let cfg = at_depth(depth, config(6, 256_000, true, FrameDuration::Ms20));
        let mut enc = MultistreamEncoder::new(&cfg, 1).expect("valid");
        let mut p = vec![];
        let mut largest = 0;
        for f in pcm.chunks_exact(enc.frame_size() * 6) {
            enc.encode(f, &mut p).expect("encodes");
            largest = largest.max(p.len());
        }
        assert!(largest <= most, "{depth} bits: {largest} bytes");
        if depth == 24 {
            assert!(largest > 4 * 4, "{largest}");
        }
    }
}

#[test]
fn a_high_band_under_the_input_depth_noise_floor_is_not_coded() {
    // Hiss at 1e-4 over loud tones: over 16 bits' noise floor in the low bands, under it
    // above 12 kHz. libopus at 64 kb/s codes all of it fullband at 24 bits, and from the
    // tenth frame on superwideband at 16.
    let pcm = tone_and_hiss(2, 0.5, 1e-4);
    for (depth, want) in [(24, Bandwidth::Full), (16, Bandwidth::SuperWide)] {
        let cfg = at_depth(depth, config(2, 64_000, true, FrameDuration::Ms20));
        let (packets, _) = encode_and_decode(&cfg, &pcm);
        let got = modes(&packets[10..]);
        assert!(
            got.iter().all(|&m| m == (Mode::Celt, want)),
            "{depth} bits: {got:?}"
        );
    }
}

#[test]
fn celt_spends_less_on_a_quiet_signal_near_the_input_depth_noise_floor() {
    // A sweep at -60 dB, fullband at 128 kb/s whatever the depth: CELT's VBR target is
    // capped by how far the loudest band stands above the noise floor of the input's depth.
    // libopus spends 20% less on it at 16 bits than at 24 (89.5 against 111.2 kb/s), and 24%
    // less on a piece of real music at -60 dB (94.2 against 123.5 kb/s).
    let pcm: Vec<f32> = signal("sweep", 2, 2.0).iter().map(|v| v * 1e-3).collect();
    let mut totals = vec![];
    for depth in [24, 16] {
        let cfg = at_depth(depth, config(2, 128_000, true, FrameDuration::Ms20));
        let (packets, _) = encode_and_decode(&cfg, &pcm);
        assert!(
            modes(&packets)
                .iter()
                .all(|&m| m == (Mode::Celt, Bandwidth::Full))
        );
        totals.push(packets.iter().map(Vec::len).sum::<usize>() as f64);
    }
    let saving = 1.0 - totals[1] / totals[0];
    assert!(saving > 0.1, "{totals:?}");
}

#[test]
fn the_same_input_at_16_bits_gives_the_same_bytes_on_every_platform() {
    // Frozen, as the 24-bit digests: a change here changes the encoder's output.
    let mut cases = vec![];
    for (channels, kind, bitrate, scale) in [
        (2u8, "music", 64_000, 1.0f32),
        (2, "sweep", 128_000, 1e-3),
        (1, "noise", 24_000, 1e-4),
    ] {
        let pcm: Vec<f32> = signal(kind, usize::from(channels), 0.5)
            .iter()
            .map(|v| v * scale)
            .collect();
        let cfg = at_depth(16, config(channels, bitrate, true, FrameDuration::Ms20));
        cases.push(digest(&cfg, &pcm));
    }
    assert_eq!(
        cases,
        [
            (5276, 12858322408569349528),
            (6763, 6916008615797558991),
            (501, 1910955883017304807)
        ]
    );
}
