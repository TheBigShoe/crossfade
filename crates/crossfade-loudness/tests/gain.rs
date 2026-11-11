use crossfade_loudness::{Gain, normalisation_gain};

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-12 * b.abs().max(1.0), "{a} vs {b}");
}

#[test]
fn gain_to_reach_the_target() {
    // -23 to -16 LUFS is +7 dB; a peak of 0.1 (-20 dBTP) leaves room.
    let g = normalisation_gain(Some(-23.0), -16.0, 0.1, -1.0);
    close(g.linear, 10f64.powf(7.0 / 20.0));
    assert!(!g.limited_by_true_peak);
}

#[test]
fn turning_down_is_never_limited() {
    let g = normalisation_gain(Some(-10.0), -23.0, 1.0, -1.0);
    close(g.linear, 10f64.powf(-13.0 / 20.0));
    assert!(!g.limited_by_true_peak);
}

#[test]
fn gain_is_limited_so_the_true_peak_stays_under_the_ceiling() {
    // +7 dB on a -6 dBTP peak would reach +1 dBTP; the -1 dBTP ceiling allows +5 dB.
    let g = normalisation_gain(Some(-23.0), -16.0, 0.5, -1.0);
    close(g.linear, 10f64.powf(-1.0 / 20.0) / 0.5);
    assert!(g.limited_by_true_peak);
    assert!(0.5 * g.linear <= 10f64.powf(-1.0 / 20.0) * (1.0 + 1e-12));
}

#[test]
fn nothing_to_measure_means_unity() {
    assert_eq!(
        normalisation_gain(None, -16.0, 0.5, -1.0),
        Gain {
            linear: 1.0,
            limited_by_true_peak: false
        }
    );
    assert_eq!(normalisation_gain(Some(f64::NAN), -16.0, 0.5, -1.0).linear, 1.0);
    assert_eq!(normalisation_gain(Some(-23.0), f64::NAN, 0.5, -1.0).linear, 1.0);
    assert_eq!(normalisation_gain(Some(f64::NEG_INFINITY), -16.0, 0.5, -1.0).linear, 1.0);
}

#[test]
fn a_silent_peak_or_a_missing_ceiling_does_not_limit() {
    let g = normalisation_gain(Some(-23.0), -16.0, 0.0, -1.0);
    close(g.linear, 10f64.powf(7.0 / 20.0));
    let g = normalisation_gain(Some(-23.0), -16.0, 0.5, f64::NAN);
    assert!(!g.limited_by_true_peak);
    let g = normalisation_gain(Some(-23.0), -16.0, 0.5, f64::INFINITY);
    assert!(!g.limited_by_true_peak);
}

#[test]
fn the_gain_is_always_finite() {
    let g = normalisation_gain(Some(-69.0), 1.0e9, 0.0, 0.0);
    assert!(g.linear.is_finite() && g.linear > 0.0);
    let g = normalisation_gain(Some(1.0e9), -23.0, 1.0, 0.0);
    assert!(g.linear.is_finite() && g.linear > 0.0);
    let g = normalisation_gain(Some(-23.0), -16.0, 1.0, f64::NEG_INFINITY);
    assert_eq!(g.linear, 0.0);
}

#[test]
fn the_same_inputs_give_the_same_bits() {
    let a = normalisation_gain(Some(-18.3), -14.0, 0.93, -1.0);
    let b = normalisation_gain(Some(-18.3), -14.0, 0.93, -1.0);
    assert_eq!(a.linear.to_bits(), b.linear.to_bits());
}
