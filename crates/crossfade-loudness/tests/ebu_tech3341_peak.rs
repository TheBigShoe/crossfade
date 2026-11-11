//! EBU Tech 3341 (2023) Table 1, true-peak cases 15-23: tolerance +0.2/-0.4 dB. Also the
//! interpolator's error over frequency and phase, and the sample peak.
mod common;

use common::*;
use crossfade_loudness::{ChannelRole, to_db};

const UP: f64 = 0.2;
const DOWN: f64 = 0.4;

fn check_dbtp(m: &crossfade_loudness::Meter, want: f64, what: &str) {
    let got = to_db(m.true_peak().unwrap());
    assert!(got <= want + UP && got >= want - DOWN, "{what}: {got} dBTP, want {want}");
}

fn sine_case(hz_div: f64, amp: f64, phase: f64, want: f64, what: &str) {
    let mut m = peak_meter(FS, &STEREO);
    m.push(&faded_sine(FS, f64::from(FS) / hz_div, amp, phase, 500, 10));
    check_dbtp(&m, want, what);
}

#[test]
fn case_15_quarter_rate_phase_0() {
    sine_case(4.0, 0.5, 0.0, -6.0, "15");
}

#[test]
fn case_16_quarter_rate_phase_45() {
    sine_case(4.0, 0.5, 45.0, -6.0, "16");
}

#[test]
fn case_17_sixth_rate_phase_60() {
    sine_case(6.0, 0.5, 60.0, -6.0, "17");
}

#[test]
fn case_18_eighth_rate_phase_67_5() {
    sine_case(8.0, 0.5, 67.5, -6.0, "18");
}

#[test]
fn case_19_quarter_rate_141_percent_phase_45() {
    sine_case(4.0, 1.41, 45.0, 3.0, "19");
}

/// Case 20-23 signal: an fs/6 sine at 0.5 with one full period of fs/4 at 1.0 inserted where
/// the carrier crosses zero, made at 4 fs, low-passed to fs/2 and decimated with `offset`.
fn inter_sample_signal(offset: usize) -> Vec<f64> {
    let up = 4usize;
    let carrier = |i: usize| 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (6.0 * up as f64)).sin();
    let mut x = Vec::new();
    // Six fs-samples of carrier end on a zero crossing; start and end of the burst are zeros too.
    for i in 0..(6 * up * 20) {
        x.push(carrier(i));
    }
    for i in 0..(4 * up) {
        x.push((2.0 * std::f64::consts::PI * i as f64 / (4.0 * up as f64)).sin());
    }
    for i in 0..(6 * up * 20) {
        x.push(carrier(i));
    }
    // Short fade in and out.
    let n = x.len();
    let fade = 6 * up * 4;
    for (i, v) in x.iter_mut().enumerate() {
        *v *= ((i + 1).min(n - i).min(fade)) as f64 / fade as f64;
    }
    // Windowed-sinc low-pass at fs/2 (a quarter of the 4 fs Nyquist), 257 taps, Hann.
    let taps = 257usize;
    let mid = (taps / 2) as f64;
    let h: Vec<f64> = (0..taps)
        .map(|k| {
            let t = k as f64 - mid;
            let sinc = if t == 0.0 { 1.0 } else { (std::f64::consts::PI * t / 4.0).sin() / (std::f64::consts::PI * t / 4.0) };
            let w = 0.5 + 0.5 * (std::f64::consts::PI * t / (mid + 1.0)).cos();
            sinc * w / 4.0
        })
        .collect();
    let filtered: Vec<f64> = (0..n)
        .map(|i| {
            h.iter()
                .enumerate()
                .map(|(k, c)| {
                    let j = i as isize + k as isize - taps as isize / 2;
                    if (0..n as isize).contains(&j) { c * x[j as usize] } else { 0.0 }
                })
                .sum()
        })
        .collect();
    filtered.iter().skip(offset).step_by(up).flat_map(|&s| [s, s]).collect()
}

fn inter_sample_case(offset: usize) {
    let mut m = peak_meter(FS, &STEREO);
    m.push(&inter_sample_signal(offset));
    check_dbtp(&m, 0.0, &format!("offset {offset}"));
}

#[test]
fn case_20_decimated_with_offset_0() {
    inter_sample_case(0);
}

#[test]
fn case_21_decimated_with_offset_1() {
    inter_sample_case(1);
}

#[test]
fn case_22_decimated_with_offset_2() {
    inter_sample_case(2);
}

#[test]
fn case_23_decimated_with_offset_3() {
    inter_sample_case(3);
}

#[test]
fn sample_peak_is_the_largest_magnitude_per_channel() {
    let roles = [ChannelRole::Left, ChannelRole::Right, ChannelRole::Lfe, ChannelRole::Excluded];
    let mut m = peak_meter(FS, &roles);
    m.push(&[0.1, -0.5, 0.25, 9.0, 0.2, 0.3, -0.75, 9.0]);
    assert_eq!(m.sample_peaks(), &[0.2, 0.5, 0.75, 0.0]);
    assert_eq!(m.sample_peak(), 0.75);
    let tp = m.true_peaks().unwrap();
    assert!(tp[0] >= 0.2 && tp[1] >= 0.5 && tp[2] >= 0.75);
    assert_eq!(tp[3], 0.0);
}

#[test]
fn true_peak_is_never_below_the_sample_peak() {
    let mut m = peak_meter(FS, &STEREO);
    m.push(&faded_sine(FS, 997.0, 0.8, 10.0, 300, 10));
    assert!(m.true_peak().unwrap() >= m.sample_peak());
}

#[test]
fn off_when_not_asked_for() {
    let mut m = meter(FS, &STEREO);
    m.push(&[0.5, 0.5]);
    assert_eq!(m.true_peak(), None);
    assert_eq!(m.true_peaks(), None);
    assert_eq!(m.sample_peak(), 0.5);
}

#[test]
fn silence_peaks_at_zero_and_minus_infinity_db() {
    let mut m = peak_meter(FS, &STEREO);
    m.push(&silence(FS, 2, 100));
    assert_eq!(m.true_peak(), Some(0.0));
    assert_eq!(to_db(0.0), f64::NEG_INFINITY);
    assert!((to_db(1.0)).abs() < 1e-12);
}

/// Worst-case error of the oversampled peak over frequency and phase. No oversampling is
/// done from 176.4 kHz up (the standard says it is not needed): there only 20 kHz-ish content
/// is judged, against the 0.44 dB a plain sample peak can miss at 19.2 kHz.
#[test]
fn oversampling_error_over_frequency_and_phase() {
    let cases: [(u32, &[f64], f64); 4] = [
        (44_100, &[8.0, 6.0, 4.0, 3.5, 3.0], 0.1),
        (48_000, &[8.0, 6.0, 4.0, 3.5, 3.0], 0.1),
        (96_000, &[8.0, 6.0, 4.0, 3.5, 3.0], 0.1),
        (192_000, &[10.0, 16.0], 0.5),
    ];
    for (rate, divs, tol) in cases {
        for &div in divs {
            let mut worst = 0.0f64;
            for phase in (0..180).step_by(3) {
                let mut m = peak_meter(rate, &[ChannelRole::Left]);
                let s: Vec<f64> =
                    faded_sine(rate, f64::from(rate) / div, 1.0, f64::from(phase), 200, 50)
                        .chunks(2)
                        .map(|p| p[0])
                        .collect();
                m.push(&s);
                worst = worst.max(db(m.true_peak().unwrap()).abs());
            }
            assert!(worst < tol, "rate {rate} fs/{div}: error {worst} dB");
        }
    }
}
