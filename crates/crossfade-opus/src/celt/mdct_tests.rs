use super::*;

/// The MDCT's definition with CELT's scaling: `y[k] = sum_n x[n] cos(pi/N2 (n + 1/2 +
/// N2/2)(k + 1/2))`, for `n` in `0..2*N2` (the window is applied by the caller).
fn forward(x: &[f64]) -> Vec<f64> {
    let n = x.len();
    let n2 = n / 2;
    (0..n2)
        .map(|k| {
            (0..n)
                .map(|j| {
                    x[j] * libm::cos(
                        std::f64::consts::PI / n2 as f64
                            * (j as f64 + 0.5 + n2 as f64 / 2.0)
                            * (k as f64 + 0.5),
                    )
                })
                .sum()
        })
        .collect()
}

#[test]
fn overlapping_frames_reconstruct_the_signal() {
    // TDAC: windowed MDCTs of overlapping frames, inverted and overlap-added, give back
    // the middle of the signal. For each long frame size (shift 0 to 3).
    for shift in 0..4 {
        let n = MDCT_N >> shift;
        let n2 = n / 2;
        let frames = 4;
        let len = n2 * (frames + 1) + OVERLAP;
        let sig: Vec<f64> = (0..len)
            .map(|i| libm::sin(i as f64 * 0.05) + 0.3 * libm::cos(i as f64 * 0.31))
            .collect();
        // CELT's window over 2*N2 samples: zero, rising over `overlap`, one, falling.
        let pad = (n2 - OVERLAP) / 2;
        let window = |j: usize| -> f64 {
            if j < pad || j >= n - pad {
                0.0
            } else if j < pad + OVERLAP {
                f64::from(WINDOW[j - pad])
            } else if j >= n - pad - OVERLAP {
                f64::from(WINDOW[n - pad - 1 - j])
            } else {
                1.0
            }
        };
        let mut out = vec![0.0f32; len + n];
        for fr in 0..frames {
            let start = fr * n2;
            let x: Vec<f64> = (0..n)
                .map(|j| sig.get(start + j).copied().unwrap_or(0.0) * window(j))
                .collect();
            let coeffs: Vec<f32> = forward(&x).iter().map(|&v| v as f32).collect();
            let mut buf = vec![0.0f32; n2 + OVERLAP];
            buf[..OVERLAP].copy_from_slice(&out[start + pad..start + pad + OVERLAP]);
            backward(&coeffs, 1, &mut buf, 0, shift);
            out[start + pad..start + pad + n2 + OVERLAP].copy_from_slice(&buf);
        }
        // CELT's IMDCT is unscaled by 2/N2... compare after fitting the scale on one sample.
        let probe = n2 + pad + OVERLAP + 7;
        let scale = sig[probe] / f64::from(out[probe]);
        for i in (n2 + pad + OVERLAP..(frames - 1) * n2 + pad).step_by(3) {
            let got = f64::from(out[i]) * scale;
            assert!(
                (got - sig[i]).abs() < 1e-3,
                "shift {shift} at {i}: {got} vs {}",
                sig[i]
            );
        }
        assert!(
            (scale * n2 as f64 / 2.0 - 1.0).abs() < 1e-3,
            "scale {scale}"
        );
    }
}

/// CELT's window over a whole block of `n` samples: zero, rising over the overlap, one,
/// falling, zero.
fn block_window(n: usize, j: usize) -> f64 {
    let pad = (n / 2 - OVERLAP) / 2;
    if j < pad || j >= n - pad {
        0.0
    } else if j < pad + OVERLAP {
        f64::from(WINDOW[j - pad])
    } else if j >= n - pad - OVERLAP {
        f64::from(WINDOW[n - pad - 1 - j])
    } else {
        1.0
    }
}

#[test]
fn the_forward_transform_is_the_windowed_mdct_scaled_by_four_over_n() {
    // What the encoder computes: `n / 2 + overlap` samples in, the block's zero ends implied.
    for shift in 0..4 {
        let n = MDCT_N >> shift;
        let n2 = n / 2;
        let pad = (n2 - OVERLAP) / 2;
        let input: Vec<f32> = (0..n2 + OVERLAP)
            .map(|i| (libm::sin(i as f64 * 0.11) + 0.4 * libm::cos(i as f64 * 0.73 + 1.0)) as f32)
            .collect();
        let block: Vec<f64> = (0..n)
            .map(|j| {
                if j < pad || j >= n - pad {
                    0.0
                } else {
                    f64::from(input[j - pad]) * block_window(n, j)
                }
            })
            .collect();
        let want = forward(&block);
        let mut got = vec![0.0f32; n2];
        super::forward(&input, &mut got, 1, shift);
        for k in 0..n2 {
            let w = want[k] * 4.0 / n as f64;
            assert!(
                (f64::from(got[k]) - w).abs() < 2e-5,
                "shift {shift} bin {k}: {} vs {w}",
                got[k]
            );
        }
    }
}

#[test]
fn the_forward_transform_interleaves_short_blocks_by_its_stride() {
    // A transient frame: 8 short MDCTs, block b's coefficients at b, b + 8, b + 16, ...
    let shift = 3;
    let n2 = (MDCT_N >> shift) / 2;
    let input: Vec<f32> = (0..n2 + OVERLAP)
        .map(|i| ((i * 37 % 101) as f32) - 50.0)
        .collect();
    let mut alone = vec![0.0f32; n2];
    super::forward(&input, &mut alone, 1, shift);
    let mut woven = vec![7.0f32; 8 * n2];
    super::forward(&input, &mut woven[3..], 8, shift);
    for k in 0..n2 {
        assert_eq!(woven[3 + 8 * k], alone[k], "bin {k}");
    }
    let untouched = woven.iter().enumerate().filter(|(i, _)| i % 8 != 3);
    assert!(untouched.clone().all(|(_, &v)| v == 7.0));
}

#[test]
fn forward_then_backward_reconstructs_overlapping_frames() {
    for shift in 0..4 {
        let n2 = (MDCT_N >> shift) / 2;
        let frames = 4;
        let sig: Vec<f32> = (0..n2 * frames + OVERLAP)
            .map(|i| (libm::sin(i as f64 * 0.05) + 0.3 * libm::cos(i as f64 * 0.31)) as f32)
            .collect();
        let mut out = vec![0.0f32; n2 * frames + OVERLAP];
        for fr in 0..frames {
            let at = fr * n2;
            let mut coeffs = vec![0.0f32; n2];
            super::forward(&sig[at..at + n2 + OVERLAP], &mut coeffs, 1, shift);
            let mut buf = vec![0.0f32; n2 + OVERLAP];
            buf[..OVERLAP].copy_from_slice(&out[at..at + OVERLAP]);
            backward(&coeffs, 1, &mut buf, 0, shift);
            out[at..at + n2 + OVERLAP].copy_from_slice(&buf);
        }
        for i in OVERLAP..n2 * frames {
            assert!(
                (out[i] - sig[i]).abs() < 1e-4,
                "shift {shift} at {i}: {} vs {}",
                out[i],
                sig[i]
            );
        }
    }
}
