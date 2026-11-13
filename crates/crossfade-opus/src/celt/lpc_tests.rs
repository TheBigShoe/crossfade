use super::*;

#[test]
fn lpc_of_an_ar1_process_finds_its_pole() {
    // ac of x[n] = 0.5 x[n-1] + e: ac[k] = 0.5^k (scaled); LPC sign convention a[0] = -0.5.
    let ac = [1.0f32, 0.5, 0.25, 0.125];
    let mut a = [0.0f32; 3];
    lpc(&mut a, &ac);
    assert!(
        (a[0] + 0.5).abs() < 1e-6 && a[1].abs() < 1e-6 && a[2].abs() < 1e-6,
        "{a:?}"
    );
}

#[test]
fn fir_then_iir_is_the_identity() {
    let coef = [0.3f32, -0.2, 0.1];
    let orig: Vec<f32> = (0..50).map(|i| ((i * 13 % 7) as f32) - 3.0).collect();
    let mut x = orig.clone();
    fir(&mut x, &coef, &mut [0.0; 3]);
    iir(&mut x, &coef, &mut [0.0; 3]);
    for (a, b) in x.iter().zip(&orig) {
        assert!((a - b).abs() < 1e-4);
    }
}

#[test]
fn the_pitch_of_a_periodic_signal_is_found() {
    // A period of 100 samples at the full rate is 50 at half rate; the search, as the
    // decoder calls it, returns poffset - pitch.
    let period = 100.0f32;
    let x: Vec<f32> = (0..2048)
        .map(|i| libm::sinf(2.0 * std::f32::consts::PI * i as f32 / period) * 1000.0)
        .collect();
    let mut lp = vec![0.0f32; 1024];
    pitch_downsample([&x, &x], 1, &mut lp);
    let poffset = 720;
    let idx = pitch_search(&lp[poffset >> 1..], &lp, 2048 - poffset, poffset - 100);
    let pitch = poffset - idx;
    assert!(pitch % 100 <= 2 || pitch % 100 >= 98, "pitch {pitch}");
}
