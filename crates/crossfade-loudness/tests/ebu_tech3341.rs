//! EBU Tech 3341 (2023) Table 1, the cases that are defined by construction: 1-6 and 9-14 for
//! loudness (tolerance +-0.1 LU), 15-23 for true peak (+0.2/-0.4 dB). Cases 7 and 8 need the
//! EBU's recorded programmes and are not covered (see the README).
mod common;

use common::*;
use crossfade_loudness::{ChannelRole, Meter};

const TOL: f64 = 0.1;

fn feed_all(m: &mut Meter, samples: &[f64]) {
    m.push(samples);
}

#[test]
fn case_01_stereo_sine_minus_23_reads_minus_23_lufs() {
    let mut m = meter(FS, &STEREO);
    feed_all(&mut m, &tone_db(FS, 2, -23.0, 20_000));
    near(m.momentary(), -23.0, TOL, "M");
    near(m.short_term(), -23.0, TOL, "S");
    near(m.integrated(), -23.0, TOL, "I");
}

#[test]
fn case_02_stereo_sine_minus_33_reads_minus_33_lufs() {
    let mut m = meter(FS, &STEREO);
    feed_all(&mut m, &tone_db(FS, 2, -33.0, 20_000));
    near(m.momentary(), -33.0, TOL, "M");
    near(m.short_term(), -33.0, TOL, "S");
    near(m.integrated(), -33.0, TOL, "I");
}

#[test]
fn case_03_relative_gate_ignores_the_quiet_ends() {
    let mut m = meter(FS, &STEREO);
    feed_all(
        &mut m,
        &steps(FS, &[(10_000, -36.0), (60_000, -23.0), (10_000, -36.0)]),
    );
    near(m.integrated(), -23.0, TOL, "I");
}

#[test]
fn case_04_absolute_and_relative_gates_together() {
    let mut m = meter(FS, &STEREO);
    let seq = [
        (10_000, -72.0),
        (10_000, -36.0),
        (60_000, -23.0),
        (10_000, -36.0),
        (10_000, -72.0),
    ];
    feed_all(&mut m, &steps(FS, &seq));
    near(m.integrated(), -23.0, TOL, "I");
}

#[test]
fn case_05_a_loud_part_that_stays_above_the_relative_gate() {
    let mut m = meter(FS, &STEREO);
    feed_all(
        &mut m,
        &steps(FS, &[(20_000, -26.0), (20_100, -20.0), (20_000, -26.0)]),
    );
    near(m.integrated(), -23.0, TOL, "I");
}

#[test]
fn case_06_five_channel_weights() {
    let roles = [
        ChannelRole::Left,
        ChannelRole::Right,
        ChannelRole::Centre,
        ChannelRole::LeftSurround,
        ChannelRole::RightSurround,
    ];
    let amps: Vec<f64> = [-28.0, -28.0, -24.0, -30.0, -30.0]
        .iter()
        .map(|&d| lin(d))
        .collect();
    let mut m = meter(FS, &roles);
    feed_all(&mut m, &tone(FS, &amps, 1000.0, 20_000));
    near(m.integrated(), -23.0, TOL, "I");
}

#[test]
fn case_09_short_term_is_constant_after_three_seconds() {
    let mut m = meter(FS, &STEREO);
    let period = steps(FS, &[(1340, -20.0), (1660, -30.0)]);
    for rep in 0..5 {
        for chunk in period.chunks(frames(FS, 100) * 2) {
            m.push(chunk);
            if rep >= 1 {
                near(m.short_term(), -23.0, TOL, "S");
            }
        }
    }
}

#[test]
fn case_10_short_term_max_per_file() {
    for i in 0..20u32 {
        let mut m = meter(FS, &STEREO);
        m.push(&silence(FS, 2, i * 150));
        m.push(&tone_db(FS, 2, -23.0, 3000));
        m.push(&silence(FS, 2, 1000));
        near(m.short_term_max(), -23.0, TOL, &format!("segment {i}"));
    }
}

#[test]
fn case_11_short_term_max_successive_values() {
    let mut m = meter(FS, &STEREO);
    for i in 0..20u32 {
        m.push(&silence(FS, 2, i * 150));
        m.push(&tone_db(FS, 2, -38.0 + f64::from(i), 3000));
        m.push(&silence(FS, 2, 3000 - i * 150));
        near(
            m.short_term_max(),
            -38.0 + f64::from(i),
            TOL,
            &format!("segment {i}"),
        );
    }
}

#[test]
fn case_12_momentary_is_constant_after_one_second() {
    let mut m = meter(FS, &STEREO);
    let period = steps(FS, &[(180, -20.0), (220, -30.0)]);
    for rep in 0..25 {
        for chunk in period.chunks(frames(FS, 20) * 2) {
            m.push(chunk);
            if rep >= 3 {
                near(m.momentary(), -23.0, TOL, "M");
            }
        }
    }
}

#[test]
fn case_13_momentary_max_per_file() {
    for i in 0..20u32 {
        let mut m = meter(FS, &STEREO);
        m.push(&silence(FS, 2, i * 20));
        m.push(&tone_db(FS, 2, -23.0, 400));
        m.push(&silence(FS, 2, 1000));
        near(m.momentary_max(), -23.0, TOL, &format!("segment {i}"));
    }
}

#[test]
fn case_14_momentary_max_successive_values() {
    let mut m = meter(FS, &STEREO);
    for i in 0..20u32 {
        m.push(&silence(FS, 2, i * 20));
        m.push(&tone_db(FS, 2, -38.0 + f64::from(i), 400));
        m.push(&silence(FS, 2, 400 - i * 20));
        near(
            m.momentary_max(),
            -38.0 + f64::from(i),
            TOL,
            &format!("segment {i}"),
        );
    }
}
