use super::*;

fn noise(n: usize, seed: u64) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((s >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
        })
        .collect()
}

/// The DCT-IV straight from its definition, in f64.
fn dct4_naive(x: &[f32]) -> Vec<f64> {
    let m = x.len();
    // cos(pi/m (n+1/2)(k+1/2)) = cos(2 pi (2n+1)(2k+1) / (8m))
    let table: Vec<f64> = (0..8 * m)
        .map(|j| libm::cos(2.0 * PI * j as f64 / (8 * m) as f64))
        .collect();
    (0..m)
        .map(|n| {
            (0..m)
                .map(|k| f64::from(x[k]) * table[((2 * n + 1) * (2 * k + 1)) % (8 * m)])
                .sum()
        })
        .collect()
}

#[test]
fn the_fast_dct_iv_matches_its_definition_at_every_vorbis_block_size() {
    for bits in 6..=13 {
        let n = 1usize << bits;
        let m = n / 2;
        let x = noise(m, bits as u64);
        let want = dct4_naive(&x);
        let imdct = Imdct::new(n);
        let mut scratch = Scratch::new(n);
        let mut z = x.clone();
        imdct.dct4(&mut z, &mut scratch);
        let peak = want.iter().fold(0.0f64, |a, v| a.max(v.abs()));
        let err = want
            .iter()
            .zip(&z)
            .fold(0.0f64, |a, (w, g)| a.max((w - f64::from(*g)).abs()));
        assert!(
            err <= 2e-6 * peak,
            "n {n}: error {err} against a peak of {peak}"
        );
    }
}

#[test]
fn unfolding_the_dct_iv_gives_the_imdct_of_the_specification() {
    for n in [64usize, 256, 2048] {
        let m = n / 2;
        let x = noise(m, n as u64 + 7);
        // y[i] = sum X[k] cos(2 pi / n (i + 1/2 + n/4)(k + 1/2))
        let want: Vec<f64> = (0..n)
            .map(|i| {
                (0..m)
                    .map(|k| {
                        f64::from(x[k])
                            * libm::cos(
                                2.0 * PI / n as f64
                                    * (i as f64 + 0.5 + n as f64 / 4.0)
                                    * (k as f64 + 0.5),
                            )
                    })
                    .sum()
            })
            .collect();
        let imdct = Imdct::new(n);
        let mut scratch = Scratch::new(n);
        let mut z = x.clone();
        imdct.dct4(&mut z, &mut scratch);
        let y = unfold(&z);
        assert_eq!(y.len(), n);
        let peak = want.iter().fold(0.0f64, |a, v| a.max(v.abs()));
        for (i, (w, g)) in want.iter().zip(&y).enumerate() {
            assert!(
                (w - f64::from(*g)).abs() <= 2e-6 * peak,
                "n {n} i {i}: {w} vs {g}"
            );
        }
    }
}

#[test]
fn a_scratch_for_the_long_block_serves_the_short_one() {
    let mut scratch = Scratch::new(2048);
    for n in [256usize, 2048, 256] {
        let x = noise(n / 2, 3);
        let mut a = x.clone();
        Imdct::new(n).dct4(&mut a, &mut scratch);
        let mut b = x.clone();
        Imdct::new(n).dct4(&mut b, &mut Scratch::new(n));
        assert_eq!(a, b);
    }
}

#[test]
fn zero_in_gives_exact_zero_out() {
    let imdct = Imdct::new(512);
    let mut scratch = Scratch::new(512);
    let mut z = vec![0.0f32; 256];
    imdct.dct4(&mut z, &mut scratch);
    assert!(z.iter().all(|&v| v == 0.0));
}
