#![allow(clippy::excessive_precision)] // libopus's outputs as it printed them

use super::*;

/// 5.1 of exact float operations (so that a C harness over libopus made the same samples):
/// a tone per channel (237, 437, ... 1237 Hz, by recursion), the rear pair 20 dB under the
/// rest, with a burst of noise every half second.
fn surround_51(frames: usize) -> Vec<f32> {
    const C2: [f32; 6] = [
        1.9990376234054565,
        1.9967286586761475,
        1.993051290512085,
        1.98800790309906,
        1.9816020727157593,
        1.973838210105896,
    ];
    const S1: [f32; 6] = [
        0.031018251553177834,
        0.05717197433114052,
        0.08328651636838913,
        0.10934397578239441,
        0.13532648980617523,
        0.16121627390384674,
    ];
    let mut osc = [[0.0f32; 2]; 6];
    let mut seed = 0x1357_9bdfu32;
    let mut out = Vec::with_capacity(frames * 960 * 6);
    for n in 0..frames * 960 {
        for c in 0..6 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = ((seed >> 8) as f32 / 16_777_216.0) * 2.0 - 1.0;
            let mut y = C2[c] * osc[c][0] - osc[c][1];
            if n == 0 {
                y = 0.0;
            }
            if n == 1 {
                y = S1[c];
            }
            osc[c][1] = osc[c][0];
            osc[c][0] = y;
            let env = (1.0 - (n % 24_000) as f32 / 1200.0).max(0.0);
            let g = if c == 3 || c == 4 { 0.025 } else { 0.25 };
            out.push(g * y + 0.05 * env * noise);
        }
    }
    out
}

fn masks(an: &mut SurroundAnalysis, pcm: &[f32], frame_size: usize) -> Vec<Vec<f32>> {
    let ch = an.channels();
    let mut out = vec![0.0f32; 21 * ch];
    pcm.chunks_exact(frame_size * ch)
        .map(|f| {
            an.analyse(f, frame_size, &mut out);
            out.clone()
        })
        .collect()
}

/// libopus 1.5.2's `surround_analysis` (float build, gcc -O2 -ffp-contract=off; a development
/// check, not run here) on frame 10 of the same signal: per channel, 21 band masks.
const FRAME_10: [[f32; 21]; 6] = [
    // FL
    [
        0.611618757,
        0.611618996,
        -0.557729244,
        -0.568086386,
        -1.02692676,
        -1.02692664,
        -1.02692652,
        -1.02692664,
        -1.02692664,
        -1.02692676,
        -1.02692676,
        -1.02692652,
        -1.02692652,
        -1.02692652,
        -1.02692652,
        -1.02692604,
        -1.02692604,
        -0.915513039,
        -0.656165123,
        -0.183145523,
        0.191604614,
    ],
    // C
    [
        1.00428712,
        1.004287,
        1.01034784,
        0.999990702,
        0.541150331,
        0.541150451,
        0.54115051,
        0.541150451,
        0.541150451,
        0.541150331,
        0.541150331,
        0.54115057,
        0.54115057,
        0.54115057,
        0.54115057,
        0.541151047,
        0.541151047,
        0.526115417,
        0.478955269,
        0.343160629,
        0.260198593,
    ],
    // FR
    [
        -0.523916841,
        -0.52391696,
        -0.523916721,
        0.614906788,
        0.610285282,
        0.369009256,
        0.369009256,
        0.369009376,
        0.369009376,
        0.369009376,
        0.369009495,
        0.369009256,
        0.369009018,
        0.369009018,
        0.369009018,
        0.369009018,
        0.369009018,
        0.369009018,
        0.369009018,
        0.369009018,
        0.344296455,
    ],
    // RL
    [
        -7.24390411,
        -7.24390411,
        -5.41325235,
        -2.4236095,
        0.117550135,
        0.117550254,
        0.117550313,
        0.117550254,
        0.117550254,
        0.117550135,
        0.117550135,
        0.117550373,
        0.117550373,
        0.117550373,
        0.117550373,
        0.11755085,
        0.11755085,
        0.102515221,
        0.055355072,
        -0.0804395676,
        -0.570044518,
    ],
    // RR
    [
        -7.15496111,
        -7.15496159,
        -7.15496111,
        -6.0161376,
        -3.02075911,
        -0.262035131,
        -0.262035131,
        -0.262035012,
        -0.262035012,
        -0.262035012,
        -0.262034893,
        -0.262035131,
        -0.26203537,
        -0.26203537,
        -0.26203537,
        -0.26203537,
        -0.26203537,
        -0.26203537,
        -0.26203537,
        -0.26203537,
        -0.286747932,
    ],
    // LFE
    [
        0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        0.0, 0.0, 0.0,
    ],
];

#[test]
fn the_masks_of_a_5_1_signal_are_libopus_s() {
    let pcm = surround_51(11);
    let got = masks(&mut SurroundAnalysis::new(6), &pcm, 960);
    for (c, want) in FRAME_10.iter().enumerate() {
        for (i, &w) in want.iter().enumerate() {
            let g = got[10][21 * c + i];
            // Log2 units (6 dB). Above 12 kHz these bands hold only the tones' leakage, 100 dB
            // down, where our MDCT's rounding and libopus's part by up to 0.004.
            let tol = if i < 17 { 1e-4 } else { 1e-2 };
            assert!((g - w).abs() < tol, "channel {c} band {i}: {g} against {w}");
        }
    }
}

#[test]
fn every_frame_size_and_hostile_input_give_finite_masks() {
    for frame_size in [120, 240, 480, 960, 1920, 2880] {
        for channels in 3..=8 {
            let mut an = SurroundAnalysis::new(channels);
            let mut pcm = surround_51(3)[..frame_size * 6].to_vec();
            pcm.resize(frame_size * channels, 0.1);
            let mut out = vec![0.0f32; 21 * channels];
            for bad in [f32::NAN, f32::INFINITY, 1e30, 0.0] {
                pcm[7] = bad;
                an.analyse(&pcm, frame_size, &mut out);
                assert!(
                    out.iter().all(|v| v.is_finite()),
                    "{frame_size} {channels} {bad}"
                );
            }
        }
    }
}

#[test]
fn reset_forgets() {
    let pcm = surround_51(6);
    let mut an = SurroundAnalysis::new(6);
    let first = masks(&mut an, &pcm, 960);
    an.reset();
    assert_eq!(masks(&mut an, &pcm, 960), first);
}
