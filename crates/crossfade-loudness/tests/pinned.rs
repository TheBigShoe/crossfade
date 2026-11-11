//! The exact bits of one fixed measurement. If these change, a normalising gain computed from
//! them changes too: that is a deliberate decision, never a side effect.
mod common;

use common::STEREO;
use crossfade_loudness::{Meter, Options};

/// Integer-only noise and a sawtooth with level steps: no transcendental function is involved
/// in making it, so it is identical everywhere.
fn programme() -> Vec<f64> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut out = Vec::new();
    for f in 0..48_000usize * 12 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let noise = ((state >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0;
        let level = [0.4, 0.04, 0.8][(f / 48_000 / 3) % 3];
        let saw = ((f % 109) as f64 / 54.0 - 1.0) * 0.5;
        out.push(level * noise);
        out.push(level * saw);
    }
    out
}

#[test]
fn a_fixed_programme_measures_to_fixed_bits() {
    let mut m = Meter::new(48_000, &STEREO, Options::default()).unwrap();
    m.push(&programme());
    let r = m.finish();
    let got = [
        r.integrated.map(f64::to_bits),
        r.momentary.map(f64::to_bits),
        r.momentary_max.map(f64::to_bits),
        r.short_term.map(f64::to_bits),
        r.short_term_max.map(f64::to_bits),
        r.loudness_range.map(f64::to_bits),
        Some(r.sample_peak.to_bits()),
        r.true_peak.map(f64::to_bits),
    ];
    let want = [
        0xc01865870f700d21, // integrated
        0xc022129a3a824eb6, // momentary
        0xc007bf6c5ac0a8b6, // momentary max
        0xc02218686dd921d8, // short-term
        0xc008115aecd8ecd2, // short-term max
        0x402690ff086acd3a, // loudness range
        0x3fe99997d45e52f5, // sample peak
        0x3ff4dc59fd736243, // true peak
    ]
    .map(Some);
    assert_eq!(got, want);
}
