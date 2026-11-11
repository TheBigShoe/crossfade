//! The K-weighting pre-filter derived for any sample rate, checked against the 48 kHz
//! coefficients printed in ITU-R BS.1770-4 (Tables 1 and 2).

use crossfade_loudness::{Error, KWeighting, MAX_SAMPLE_RATE, MIN_SAMPLE_RATE};

const SHELF_B: [f64; 3] = [1.53512485958697, -2.69169618940638, 1.19839281085285];
const SHELF_A: [f64; 3] = [1.0, -1.69065929318241, 0.73248077421585];
const HP_B: [f64; 3] = [1.0, -2.0, 1.0];
const HP_A: [f64; 3] = [1.0, -1.99004745483398, 0.99007225036621];

/// Reference evaluator, std maths on purpose: independent of the crate's own.
fn db(b: &[f64; 3], a: &[f64; 3], rate: f64, hz: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * hz / rate;
    let z = |k: f64| (f64::cos(k * w), -f64::sin(k * w));
    let poly = |c: &[f64; 3]| {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, v) in c.iter().enumerate() {
            let (r, j) = z(i as f64);
            re += v * r;
            im += v * j;
        }
        re * re + im * im
    };
    10.0 * f64::log10(poly(b) / poly(a))
}

#[test]
fn coefficients_at_48k_equal_the_published_ones() {
    let k = KWeighting::new(48_000).unwrap();
    for i in 0..3 {
        assert!((k.shelf.b[i] - SHELF_B[i]).abs() < 1e-9, "shelf b{i}");
        assert!((k.shelf.a[i] - SHELF_A[i]).abs() < 1e-9, "shelf a{i}");
        assert!((k.highpass.b[i] - HP_B[i]).abs() < 1e-9, "hp b{i}");
        assert!((k.highpass.a[i] - HP_A[i]).abs() < 1e-9, "hp a{i}");
    }
}

#[test]
fn response_at_48k_matches_the_published_filter() {
    let k = KWeighting::new(48_000).unwrap();
    for hz in [10.0, 38.0, 100.0, 1000.0, 1681.0, 4000.0, 10_000.0, 20_000.0] {
        let want = db(&SHELF_B, &SHELF_A, 48e3, hz) + db(&HP_B, &HP_A, 48e3, hz);
        assert!((k.gain_db(hz) - want).abs() < 1e-6, "{hz} Hz");
    }
}

#[test]
fn response_has_the_shape_of_the_standard_curve() {
    let k = KWeighting::new(48_000).unwrap();
    // 1 kHz sits +0.7 dB up the shelf, which is what the -0.691 LUFS offset compensates.
    assert!((k.gain_db(1000.0) - 0.691).abs() < 0.03);
    // The shelf tops out near +4 dB, the revised low-frequency B-curve rolls off below 100 Hz.
    assert!((k.gain_db(10_000.0) - 4.0).abs() < 0.4);
    assert!(k.gain_db(20.0) < -2.0);
    assert!(k.gain_db(10.0) < -10.0);
}

#[test]
fn other_rates_keep_the_analogue_response() {
    // One analogue prototype at any rate: the bilinear warping leaves under 0.06 dB below 3 kHz.
    let base = KWeighting::new(192_000).unwrap();
    for rate in [44_100, 96_000, 176_400, 1_048_575] {
        let k = KWeighting::new(rate).unwrap();
        for hz in [50.0, 500.0, 1000.0, 3000.0] {
            let d = (k.gain_db(hz) - base.gain_db(hz)).abs();
            assert!(d < 0.06, "{rate} {hz} {d}");
        }
    }
}

#[test]
fn every_supported_rate_gives_stable_filters() {
    for rate in [MIN_SAMPLE_RATE, 11_025, 22_050, 32_000, 44_100, 48_000, 96_000, 192_000, 384_000, MAX_SAMPLE_RATE] {
        let k = KWeighting::new(rate).unwrap();
        for f in [k.shelf, k.highpass] {
            assert!(f.a[2].abs() < 1.0, "{rate}");
            assert!(f.a[1].abs() < 1.0 + f.a[2], "{rate}");
        }
    }
}

#[test]
fn rates_outside_the_range_are_errors() {
    assert_eq!(
        KWeighting::new(MIN_SAMPLE_RATE - 1).unwrap_err(),
        Error::SampleRate { rate: 7_999 }
    );
    assert_eq!(KWeighting::new(0).unwrap_err(), Error::SampleRate { rate: 0 });
    assert_eq!(
        KWeighting::new(MAX_SAMPLE_RATE + 1).unwrap_err(),
        Error::SampleRate { rate: 1_048_576 }
    );
    assert!(Error::SampleRate { rate: 5 }.to_string().contains("5"));
}
