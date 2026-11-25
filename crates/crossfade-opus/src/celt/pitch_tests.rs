use super::*;

/// `len` samples of a voiced-like signal of period `period` (a decaying pulse train through a
/// resonance, CELT's scale), or noise.
fn voiced(len: usize, period: usize, seed: u32) -> Vec<f32> {
    let mut s = seed;
    let mut y1 = 0.0f32;
    let mut y2 = 0.0f32;
    (0..len)
        .map(|i| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = ((s >> 8) as f32 / (1 << 24) as f32) - 0.5;
            let x = if period > 0 && i % period == 0 {
                8000.0
            } else {
                0.0
            } + 50.0 * noise;
            // A resonance near 700 Hz.
            let y = x + 1.8 * y1 - 0.9 * y2;
            y2 = y1;
            y1 = y;
            y
        })
        .collect()
}

/// The pre-filter's pitch analysis of one frame of `n` after `COMBFILTER_MAXPERIOD` of
/// history, as `run_prefilter` does it: the period and the gain.
fn analyse(x: &[f32], n: usize) -> (usize, f32) {
    let len = 1024 + n;
    let mut lp = vec![0.0f32; len >> 1];
    pitch_downsample([&x[..len], &x[..len]], 1, &mut lp);
    let found = pitch_search(&lp[512..], &lp, n, 1024 - 45);
    let mut period = 1024 - found;
    let gain = remove_doubling(&lp, 1024, 15, n, &mut period, 0, 0.0);
    (period, gain)
}

#[test]
fn a_periodic_signal_gives_its_period_and_a_high_gain() {
    for period in [40, 97, 160, 300, 512] {
        let x = voiced(1024 + 960, period, 7);
        let (found, gain) = analyse(&x, 960);
        assert!(found.abs_diff(period) <= 2, "{period}: {found}");
        assert!(gain > 0.6, "{period}: gain {gain}");
    }
}

#[test]
fn noise_gives_a_low_gain() {
    let x = voiced(1024 + 480, 0, 3);
    let (_, gain) = analyse(&x, 480);
    assert!(gain < 0.3, "gain {gain}");
}

#[test]
fn silence_gives_no_gain_and_a_period_in_range() {
    let x = vec![0.0f32; 1024 + 240];
    let (period, gain) = analyse(&x, 240);
    assert_eq!(gain, 0.0);
    assert!((15..=1024).contains(&period));
}
