//! EBU Tech 3342 (2023) Table 1, the synthetic loudness-range cases 1-4 (+-1 LU). Cases 5
//! and 6 are recorded programmes and are not covered (see the README).
mod common;

use common::*;
use crossfade_loudness::{Meter, Options};

fn lra_meter() -> Meter {
    Meter::new(
        FS,
        &STEREO,
        Options {
            true_peak: false,
            loudness_range: true,
        },
    )
    .unwrap()
}

fn lra(seq: &[(u32, f64)]) -> Option<f64> {
    let mut m = lra_meter();
    m.push(&steps(FS, seq));
    m.loudness_range()
}

#[test]
fn case_1_two_tones_10_db_apart() {
    near(lra(&[(20_000, -20.0), (20_000, -30.0)]), 10.0, 1.0, "LRA");
}

#[test]
fn case_2_two_tones_5_db_apart() {
    near(lra(&[(20_000, -20.0), (20_000, -15.0)]), 5.0, 1.0, "LRA");
}

#[test]
fn case_3_two_tones_20_db_apart() {
    near(lra(&[(20_000, -40.0), (20_000, -20.0)]), 20.0, 1.0, "LRA");
}

#[test]
fn case_4_five_tones() {
    let seq = [-50.0, -35.0, -20.0, -35.0, -50.0].map(|d| (20_000, d));
    near(lra(&seq), 15.0, 1.0, "LRA");
}

#[test]
fn repeating_the_signal_leaves_the_range_alone() {
    let once = [(20_000, -20.0), (20_000, -30.0)];
    let mut m = lra_meter();
    for _ in 0..3 {
        m.push(&steps(FS, &once));
    }
    near(m.loudness_range(), 10.0, 1.0, "LRA x3");
}

#[test]
fn a_steady_tone_has_no_range() {
    near(lra(&[(10_000, -23.0)]), 0.0, 0.01, "LRA");
}

#[test]
fn silence_and_short_input_have_no_range() {
    assert_eq!(lra(&[(10_000, -90.0)]), None);
    assert_eq!(lra(&[(2_900, -23.0)]), None);
    assert_eq!(meter(FS, &STEREO).loudness_range(), None);
}

#[test]
fn switched_off_it_reports_none() {
    let mut m = meter(FS, &STEREO);
    m.push(&steps(FS, &[(10_000, -23.0)]));
    assert_eq!(m.loudness_range(), None);
}

#[test]
fn the_relative_gate_drops_blocks_20_lu_under() {
    // 30 dB under the loud part is gated out (only the windows straddling the step stay, so the
    // range is far below the 30 dB it would be ungated); 15 dB under counts.
    let gated = lra(&[(30_000, -20.0), (30_000, -50.0)]).unwrap();
    assert!(gated < 15.0, "{gated}");
    near(lra(&[(30_000, -20.0), (30_000, -35.0)]), 15.0, 1.0, "not gated");
}
