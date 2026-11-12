//! The synthesis filterbank (14496-3 4.6.11): IMDCT of 2048 and 256 points, the sine and
//! Kaiser-Bessel-derived windows, block switching and overlap-add.
//!
//! The IMDCT is a DCT-IV of N/2 points folded out to N; the DCT-IV runs as a complex FFT of
//! N/4 points between two twiddles (even inputs real, odd inputs imaginary, reversed). Tables
//! are built once with `libm` in `f64`, so they are the same bits everywhere; the transform is
//! `f32` with no fused multiply-add.

use std::f64::consts::PI;
use std::sync::OnceLock;

pub(crate) const ONLY_LONG: u8 = 0;
pub(crate) const LONG_START: u8 = 1;
pub(crate) const EIGHT_SHORT: u8 = 2;
pub(crate) const LONG_STOP: u8 = 3;

/// An inverse MDCT of `2 * m` outputs from `m` coefficients.
pub(crate) struct Imdct {
    m: usize,
    /// Pre- and post-twiddles `e^(-i pi (k + 1/8) / m)`, k < m/2; the post ones carry the
    /// output scale.
    pre: Vec<(f32, f32)>,
    post: Vec<(f32, f32)>,
    fft: Fft,
}

impl Imdct {
    fn new(m: usize, scale: f64) -> Self {
        let tw = |k: usize, s: f64| {
            let a = -PI * (k as f64 + 0.125) / m as f64;
            ((libm::cos(a) * s) as f32, (libm::sin(a) * s) as f32)
        };
        Self {
            m,
            pre: (0..m / 2).map(|k| tw(k, 1.0)).collect(),
            post: (0..m / 2).map(|k| tw(k, scale)).collect(),
            fft: Fft::new(m / 2),
        }
    }

    /// `out` (`2 * m` samples) from `spec` (`m` coefficients); `buf` is scratch of `m / 2`.
    pub(crate) fn run(&self, spec: &[f32], out: &mut [f32], buf: &mut [(f32, f32)]) {
        let m = self.m;
        let (q, h) = (m / 2, m / 2);
        let buf = &mut buf[..q];
        for (k, (b, &(c, s))) in buf.iter_mut().zip(&self.pre).enumerate() {
            let (re, im) = (spec[2 * k], spec[m - 1 - 2 * k]);
            *b = (re * c - im * s, re * s + im * c);
        }
        self.fft.run(buf);
        // u[2n] = Re, u[m - 1 - 2n] = -Im; then the fold:
        // y[n] = u[n + m/2] (n < m/2), -u[3m/2 - 1 - n] (n < 3m/2), -u[n - 3m/2].
        let (first, rest) = out.split_at_mut(h);
        let (middle, last) = rest.split_at_mut(m);
        for (n, (b, &(c, s))) in buf.iter().zip(&self.post).enumerate() {
            let (re, im) = (b.0 * c - b.1 * s, b.0 * s + b.1 * c);
            let even = re; // u[2n]
            let odd = -im; // u[m - 1 - 2n]
            // u[j] lands at y[j - m/2] when j >= m/2, at y[3m/2 - 1 - j] (negated) always,
            // and at y[j + 3m/2] (negated) when j < m/2.
            for (j, v) in [(2 * n, even), (m - 1 - 2 * n, odd)] {
                middle[m - 1 - j] = -v;
                if j >= h {
                    first[j - h] = v;
                } else {
                    last[j] = -v;
                }
            }
        }
    }
}

/// Radix-2 complex FFT, forward (`e^(-2 pi i nk / n)`), in place.
struct Fft {
    n: usize,
    rev: Vec<u16>,
    twiddle: Vec<(f32, f32)>,
}

impl Fft {
    fn new(n: usize) -> Self {
        let bits = n.trailing_zeros();
        let rev = (0..n)
            .map(|i| (i.reverse_bits() >> (usize::BITS - bits)) as u16)
            .collect();
        let twiddle = (0..n / 2)
            .map(|k| {
                let a = -2.0 * PI * k as f64 / n as f64;
                (libm::cos(a) as f32, libm::sin(a) as f32)
            })
            .collect();
        Self { n, rev, twiddle }
    }

    fn run(&self, x: &mut [(f32, f32)]) {
        let n = self.n;
        for i in 0..n {
            let j = usize::from(self.rev[i]);
            if i < j {
                x.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for block in x.chunks_exact_mut(len) {
                let (a, b) = block.split_at_mut(half);
                for (k, (u, v)) in a.iter_mut().zip(b.iter_mut()).enumerate() {
                    let (c, s) = self.twiddle[k * step];
                    let t = (v.0 * c - v.1 * s, v.0 * s + v.1 * c);
                    *v = (u.0 - t.0, u.1 - t.1);
                    *u = (u.0 + t.0, u.1 + t.1);
                }
            }
            len *= 2;
        }
    }
}

/// The rising halves of the windows: `[sine, KBD]`, long (1024) and short (128).
pub(crate) struct Windows {
    pub(crate) long: [Vec<f32>; 2],
    pub(crate) short: [Vec<f32>; 2],
}

fn sine(n: usize) -> Vec<f32> {
    (0..n / 2)
        .map(|i| libm::sin(PI / n as f64 * (i as f64 + 0.5)) as f32)
        .collect()
}

/// Modified Bessel function of the first kind, order 0, by its series.
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    loop {
        term *= (x / (2.0 * k)) * (x / (2.0 * k));
        sum += term;
        if term < sum * 1e-20 {
            return sum;
        }
        k += 1.0;
    }
}

/// The Kaiser-Bessel-derived window (14496-3 4.6.11.3.2) of length `n`, rising half.
fn kbd(n: usize, alpha: f64) -> Vec<f32> {
    let half = n / 2;
    let kaiser: Vec<f64> = (0..=half)
        .map(|j| {
            let r = 2.0 * j as f64 / half as f64 - 1.0;
            bessel_i0(PI * alpha * libm::sqrt(1.0 - r * r))
        })
        .collect();
    let total: f64 = kaiser.iter().sum();
    let mut acc = 0.0;
    (0..half)
        .map(|i| {
            acc += kaiser[i];
            libm::sqrt(acc / total) as f32
        })
        .collect()
}

pub(crate) struct Tables {
    pub(crate) long: Imdct,
    pub(crate) short: Imdct,
    pub(crate) windows: Windows,
}

/// Built once. The IMDCT's 2/N and the scale from the standard's 16-bit range to -1..1 are
/// folded into the post-twiddles.
pub(crate) fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| Tables {
        long: Imdct::new(1024, 2.0 / 2048.0 / 32768.0),
        short: Imdct::new(128, 2.0 / 256.0 / 32768.0),
        windows: Windows {
            long: [sine(2048), kbd(2048, 4.0)],
            short: [sine(256), kbd(256, 6.0)],
        },
    })
}

/// One channel's filterbank state: the second half of the previous frame's windowed output,
/// and its window shape.
pub(crate) struct Overlap {
    pub(crate) saved: Vec<f32>,
    pub(crate) shape: u8,
}

impl Overlap {
    pub(crate) fn new() -> Self {
        Self {
            saved: vec![0.0; 1024],
            shape: 0,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.saved.fill(0.0);
        self.shape = 0;
    }
}

/// Scratch the filterbank reuses frame to frame.
pub(crate) struct Scratch {
    time: Vec<f32>,
    short: Vec<f32>,
    fft: Vec<(f32, f32)>,
}

impl Scratch {
    pub(crate) fn new() -> Self {
        Self {
            time: vec![0.0; 2048],
            short: vec![0.0; 256],
            fft: vec![(0.0, 0.0); 512],
        }
    }
}

/// Turns one frame's spectrum into 1024 output samples (`out`), overlapping with the previous
/// frame and keeping this frame's second half for the next.
pub(crate) fn synthesize(
    spec: &[f32],
    sequence: u8,
    shape: u8,
    state: &mut Overlap,
    scratch: &mut Scratch,
    out: &mut [f32],
) {
    let t = tables();
    let prev = usize::from(state.shape);
    let cur = usize::from(shape);
    let (wl_prev, wl_cur) = (&t.windows.long[prev], &t.windows.long[cur]);
    let (ws_prev, ws_cur) = (&t.windows.short[prev], &t.windows.short[cur]);
    let time = &mut scratch.time;
    if sequence == EIGHT_SHORT {
        time.fill(0.0);
        for w in 0..8 {
            t.short.run(
                &spec[w * 128..(w + 1) * 128],
                &mut scratch.short,
                &mut scratch.fft,
            );
            let rise = if w == 0 { ws_prev } else { ws_cur };
            let at = 448 + 128 * w;
            let (head, tail) = scratch.short.split_at(128);
            for (i, (&x, &wv)) in head.iter().zip(rise.iter()).enumerate() {
                time[at + i] += x * wv;
            }
            for (i, (&x, &wv)) in tail.iter().zip(ws_cur.iter().rev()).enumerate() {
                time[at + 128 + i] += x * wv;
            }
        }
    } else {
        t.long.run(spec, time, &mut scratch.fft);
        let (head, tail) = time.split_at_mut(1024);
        if sequence == LONG_STOP {
            head[..448].fill(0.0);
            for (x, &wv) in head[448..576].iter_mut().zip(ws_prev.iter()) {
                *x *= wv;
            }
        } else {
            for (x, &wv) in head.iter_mut().zip(wl_prev.iter()) {
                *x *= wv;
            }
        }
        if sequence == LONG_START {
            for (x, &wv) in tail[448..576].iter_mut().zip(ws_cur.iter().rev()) {
                *x *= wv;
            }
            tail[576..].fill(0.0);
        } else {
            for (x, &wv) in tail.iter_mut().zip(wl_cur.iter().rev()) {
                *x *= wv;
            }
        }
    }
    for ((o, &a), &b) in out.iter_mut().zip(&time[..1024]).zip(&state.saved) {
        *o = a + b;
    }
    state.saved.copy_from_slice(&time[1024..]);
    state.shape = shape;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The standard's definition, in f64.
    fn direct(spec: &[f32]) -> Vec<f64> {
        let n = 2 * spec.len();
        let n0 = (n as f64 / 2.0 + 1.0) / 2.0;
        (0..n)
            .map(|i| {
                spec.iter()
                    .enumerate()
                    .map(|(k, &x)| {
                        f64::from(x)
                            * libm::cos(2.0 * PI / n as f64 * (i as f64 + n0) * (k as f64 + 0.5))
                    })
                    .sum::<f64>()
                    * 2.0
                    / n as f64
            })
            .collect()
    }

    #[test]
    fn the_fast_imdct_is_the_standards_definition() {
        for m in [1024usize, 128] {
            let spec: Vec<f32> = (0..m)
                .map(|k| ((k * 7919 % 1000) as f32 - 500.0) * 3.0)
                .collect();
            let imdct = Imdct::new(m, 2.0 / (2 * m) as f64);
            let mut out = vec![0.0; 2 * m];
            let mut buf = vec![(0.0, 0.0); m / 2];
            imdct.run(&spec, &mut out, &mut buf);
            let want = direct(&spec);
            let peak = want.iter().fold(0.0f64, |a, b| a.max(b.abs()));
            for (i, (&g, w)) in out.iter().zip(&want).enumerate() {
                assert!(
                    (f64::from(g) - w).abs() < peak * 1e-5,
                    "{m} {i}: {g} vs {w}"
                );
            }
        }
    }

    #[test]
    fn windows_are_power_complementary() {
        let t = tables();
        for w in t.windows.long.iter().chain(&t.windows.short) {
            let n = w.len();
            for i in 0..n {
                let (a, b) = (f64::from(w[i]), f64::from(w[n - 1 - i]));
                assert!((a * a + b * b - 1.0).abs() < 1e-6, "{i}");
            }
        }
        // KBD rises slower at the start than sine.
        assert!(t.windows.long[1][10] < t.windows.long[0][10]);
    }

    #[test]
    fn overlap_adding_long_frames_reconstructs_a_steady_spectrum() {
        // The same spectrum frame after frame: the output is the same frame after frame.
        let mut spec = vec![0.0; 1024];
        spec[10] = 1000.0;
        let mut state = Overlap::new();
        let mut scratch = Scratch::new();
        let mut a = vec![0.0; 1024];
        let mut b = vec![0.0; 1024];
        synthesize(&spec, ONLY_LONG, 0, &mut state, &mut scratch, &mut a);
        synthesize(&spec, ONLY_LONG, 0, &mut state, &mut scratch, &mut a);
        synthesize(&spec, ONLY_LONG, 0, &mut state, &mut scratch, &mut b);
        assert_eq!(a, b);
    }
}
