//! Results do not depend on how the input is cut into `push` calls.
mod common;

use crossfade_loudness::{ChannelRole, Meter, Options};
use proptest::prelude::*;

/// Deterministic noise with level steps, so gates and peaks all see something.
fn signal(seed: u64, channels: usize, frames: usize) -> Vec<f64> {
    let mut state = seed | 1;
    let mut next = move || {
        state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((state >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    };
    let mut out = Vec::with_capacity(frames * channels);
    for f in 0..frames {
        let level = [0.5, 0.05, 0.9, 0.0005][(f / 9_000) % 4];
        for _ in 0..channels {
            out.push(level * next());
        }
    }
    out
}

const ROLES: [ChannelRole; 6] = [
    ChannelRole::Left,
    ChannelRole::Right,
    ChannelRole::Centre,
    ChannelRole::Lfe,
    ChannelRole::LeftSurround,
    ChannelRole::RightSurround,
];
const RATES: [u32; 6] = [8_000, 11_025, 44_100, 48_000, 96_000, 192_000];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn chunking_never_changes_a_bit(
        seed in any::<u64>(),
        rate in prop::sample::select(RATES.to_vec()),
        channels in 1usize..=6,
        cuts in prop::collection::vec(0usize..40_000, 1..12),
        lra in any::<bool>(),
        tp in any::<bool>(),
    ) {
        let options = Options { true_peak: tp, loudness_range: lra };
        let roles = &ROLES[..channels];
        let frames = 40_000usize.min(rate as usize * 4);
        let samples = signal(seed, channels, frames);

        let mut whole = Meter::new(rate, roles, options).unwrap();
        whole.push(&samples);

        let mut parts = Meter::new(rate, roles, options).unwrap();
        let mut at = 0;
        for cut in &cuts {
            let end = (at + cut).min(samples.len());
            parts.push(&samples[at..end]);
            at = end;
        }
        parts.push(&samples[at..]);

        let (a, b) = (whole.measurements(), parts.measurements());
        prop_assert_eq!(&a, &b);
        prop_assert_eq!(whole.true_peaks().map(|v| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>()),
                        parts.true_peaks().map(|v| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>()));
        for v in [a.integrated, a.momentary, a.short_term, a.loudness_range].into_iter().flatten() {
            prop_assert!(v.is_finite());
        }
    }

    #[test]
    fn a_readout_in_the_middle_does_not_disturb_the_stream(seed in any::<u64>(), at in 0usize..30_000) {
        let samples = signal(seed, 2, 30_000);
        let mut quiet = Meter::new(48_000, &common::STEREO, Options::default()).unwrap();
        quiet.push(&samples);
        let mut peeked = Meter::new(48_000, &common::STEREO, Options::default()).unwrap();
        peeked.push(&samples[..at * 2]);
        let _ = peeked.measurements();
        peeked.push(&samples[at * 2..]);
        prop_assert_eq!(quiet.measurements(), peeked.measurements());
    }
}
