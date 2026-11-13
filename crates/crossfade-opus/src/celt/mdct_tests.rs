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
