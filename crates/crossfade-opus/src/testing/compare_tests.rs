use super::*;

/// Deterministic white noise, uniform in [-1, 1) (variance 1/3), from xorshift64.
fn noise(n: usize, seed: u64) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 40) as f32 / (1u64 << 23) as f32) - 1.0
        })
        .collect()
}

fn scaled(v: &[f32], g: f32) -> Vec<f32> {
    v.iter().map(|&s| s * g).collect()
}

fn q_from_err(err: f64) -> f32 {
    (100.0 * (1.0 - 0.5 * libm::log(1.0 + err) / libm::log(1.13))) as f32
}

/// 0.2 s of stereo at 48 kHz: 77 analysis frames.
const N: usize = 2 * 9600;

#[test]
fn a_signal_against_itself_gives_the_maximum() {
    let x = scaled(&noise(N, 1), 8000.0);
    let q = compare(&x, &x, true, 48000).unwrap();
    assert_eq!(q.err, 0.0);
    assert_eq!(q.q, 100.0);
    assert!(q.passes());
    // Mono: the reference is averaged to mono first.
    let mono: Vec<f32> = x
        .chunks(2)
        .map(|c| (0.5 * (f64::from(c[0]) + f64::from(c[1]))) as f32)
        .collect();
    assert_eq!(compare(&x, &mono, false, 48000).unwrap().q, 100.0);
    let pcm: Vec<i16> = x.iter().map(|&s| s as i16).collect();
    assert_eq!(compare_pcm16(&pcm, &pcm, true, 48000).unwrap().q, 100.0);
}

#[test]
fn small_added_noise_on_silence_follows_the_square_law() {
    // Against silence, the ratio Y/X - 1 is the noise power over the floor, so it scales with
    // the noise power; im = r - ln r - 1 ~ (r - 1)^2 / 2, and err is the 16th root of the
    // 8th power of im's band means, so err ~ (r - 1): halving the noise amplitude quarters err.
    let silence = vec![0f32; N];
    let n = noise(N, 7);
    let loud = compare(&silence, &scaled(&n, 1.0), true, 48000).unwrap();
    let quiet = compare(&silence, &scaled(&n, 0.5), true, 48000).unwrap();
    let ratio = loud.err / quiet.err;
    assert!((3.95..=4.05).contains(&ratio), "err ratio {ratio}");
    assert!((quiet.q - q_from_err(loud.err / 4.0)).abs() < 0.05);
    assert!(quiet.q > loud.q);
}

#[test]
fn small_noise_on_a_loud_signal_follows_the_linear_law() {
    // Against a loud reference, Y/X - 1 is dominated by the cross term 2 Re(X N*) / |X|^2,
    // linear in the noise amplitude: halving it halves err.
    let x = scaled(&noise(N, 3), 12000.0);
    let n = noise(N, 11);
    let with = |g: f32| -> Vec<f32> { x.iter().zip(&n).map(|(a, b)| a + g * b).collect() };
    let loud = compare(&x, &with(4.0), true, 48000).unwrap();
    let quiet = compare(&x, &with(2.0), true, 48000).unwrap();
    let ratio = loud.err / quiet.err;
    assert!((1.95..=2.05).contains(&ratio), "err ratio {ratio}");
    assert!((quiet.q - q_from_err(loud.err / 2.0)).abs() < 0.05);
}

#[test]
fn white_noise_at_minus_10_db_fails_and_at_minus_40_db_passes() {
    // Measured once and pinned, against a white-noise reference (the err ratios between these
    // levels follow the linear law above: -40 to -60 dB divides err by 10.1).
    let x = scaled(&noise(N, 5), 10000.0);
    let n = noise(N, 9);
    let at = |db: f64| {
        let g = 10000.0 * libm::pow(10.0, db / 20.0) as f32;
        let y: Vec<f32> = x.iter().zip(&n).map(|(a, b)| a + g * b).collect();
        compare(&x, &y, true, 48000).unwrap()
    };
    let bad = at(-10.0);
    let good = at(-40.0);
    assert!(!bad.passes(), "{bad:?}");
    assert!((bad.q - -44.78).abs() < 0.01, "{bad:?}");
    assert!(good.passes() && (good.q - 95.39).abs() < 0.01, "{good:?}");
    // The pass mark, Q = 0, is err = 1.13^2 - 1.
    assert!(q_from_err(1.13f64 * 1.13 - 1.0).abs() < 1e-6);
}

#[test]
fn the_tools_errors() {
    let x = vec![0f32; N];
    assert_eq!(
        compare(&x, &x[..N - 2], true, 48000).unwrap_err(),
        "Sample counts do not match (9600!=9599)."
    );
    assert_eq!(
        compare(&x[..958], &x[..958], true, 48000).unwrap_err(),
        "Insufficient sample data (479<480)."
    );
    assert!(compare(&x, &x, true, 44100).is_err());
    // At 24 kHz the decoded file has half the samples.
    let y = vec![0f32; N / 2];
    assert!(compare(&x, &y, true, 24000).unwrap().passes());
}

#[test]
fn it_is_deterministic() {
    let x = scaled(&noise(N, 21), 9000.0);
    let y = scaled(&noise(N, 21), 8000.0);
    let a = compare(&x, &y, true, 48000).unwrap();
    let b = compare(&x, &y, true, 48000).unwrap();
    assert_eq!(a.q.to_bits(), b.q.to_bits());
    assert_eq!(a.err.to_bits(), b.err.to_bits());
}
