//! Polyphase coefficient tables: every phase held exactly when they fit, otherwise a fixed
//! number of rows per sample with cubic (4-point Lagrange) interpolation between them.

use crate::design::Kernel;
use crate::{Sample, kernel};

/// The most coefficients an exact table may hold (2 MiB of `f64`). Past it, phases are
/// interpolated from `rows_per_sample + 3` rows, whatever the rates.
pub(crate) const EXACT_MAX: usize = 1 << 18;

#[derive(Debug, Clone)]
pub(crate) struct Table<S> {
    taps: usize,
    rows: Vec<S>,
    /// `None` for an exact table (row `p` is phase `p`); otherwise the rows per sample, with
    /// row `r + 1` at position `r / per` for `r` from -1 to `per + 1`.
    per: Option<u64>,
    scratch: Vec<S>,
}

/// How rows are scaled so that DC passes with gain 1.
pub(crate) enum Norm {
    /// Every row sums to 1: the direct form, where one output uses one row.
    Rows,
    /// Coefficients are grouped by their kernel position `m / phases` modulo `classes`, and
    /// each group sums to 1: the transposed form, where one output takes one tap from many
    /// rows. Exact tables only.
    Classes(u64),
    /// Scale everything by this: an interpolated transposed form, normalised by the
    /// kernel's integral (see [`Table::integral`]).
    Scale(f64),
}

impl<S: Sample> Table<S> {
    /// `phases` rows at positions `p / phases`, or interpolated rows when that is too many.
    pub(crate) fn new(
        kernel: &Kernel,
        taps: usize,
        phases: u64,
        rows_per_sample: u64,
        norm: impl FnOnce(&[f64], Option<u64>) -> Norm,
    ) -> Self {
        let exact = phases.saturating_mul(taps as u64) <= EXACT_MAX as u64;
        let (mut rows, per) = if exact {
            let r = rows(kernel, taps, phases as usize, |p| p as f64 / phases as f64);
            (r, None)
        } else {
            let per = rows_per_sample;
            let r = rows(kernel, taps, per as usize + 3, |r| {
                (r as f64 - 1.0) / per as f64
            });
            (r, Some(per))
        };
        match norm(&rows, per) {
            Norm::Rows => {
                for row in rows.chunks_exact_mut(taps) {
                    let sum: f64 = row.iter().sum();
                    row.iter_mut().for_each(|c| *c /= sum);
                }
            }
            Norm::Classes(classes) => {
                let mut sums = vec![0.0; classes as usize];
                let class = |p: usize, k: usize| {
                    let m = (k as i64 + 1 - (taps / 2) as i64) * phases as i64 - p as i64;
                    m.rem_euclid(classes as i64) as usize
                };
                for (p, row) in rows.chunks_exact(taps).enumerate() {
                    for (k, c) in row.iter().enumerate() {
                        sums[class(p, k)] += c;
                    }
                }
                for (p, row) in rows.chunks_exact_mut(taps).enumerate() {
                    for (k, c) in row.iter_mut().enumerate() {
                        *c /= sums[class(p, k)];
                    }
                }
            }
            Norm::Scale(s) => rows.iter_mut().for_each(|c| *c *= s),
        }
        Self {
            taps,
            rows: rows.into_iter().map(S::from_f64).collect(),
            per,
            scratch: vec![S::ZERO; taps],
        }
    }

    pub(crate) fn is_exact(&self) -> bool {
        self.per.is_none()
    }

    pub(crate) fn len(&self) -> usize {
        self.rows.len() + self.scratch.len()
    }

    /// The coefficients for position `num / den` of a sample (`num < den`).
    pub(crate) fn coefs(&mut self, num: u64, den: u64) -> &[S] {
        let taps = self.taps;
        let Some(per) = self.per else {
            let at = num as usize * taps;
            return &self.rows[at..at + taps];
        };
        let x = u128::from(num) * u128::from(per);
        let r = (x / u128::from(den)) as usize;
        let rem = (x % u128::from(den)) as u64;
        if rem == 0 {
            let at = (r + 1) * taps;
            return &self.rows[at..at + taps];
        }
        let t = rem as f64 / den as f64;
        let w = lagrange(t).map(S::from_f64);
        let block = &self.rows[r * taps..(r + 4) * taps];
        let (r0, rest) = block.split_at(taps);
        let (r1, rest) = rest.split_at(taps);
        let (r2, r3) = rest.split_at(taps);
        kernel::blend4(&mut self.scratch, w, [r0, r1, r2, r3]);
        &self.scratch
    }
}

/// Integral of the kernel the interpolated rows represent: the sum over one sample of
/// positions of every tap, integrating each cubic piece exactly. `rows` is an interpolated
/// table as built (row `r + 1` at position `r / per`).
pub(crate) fn integral(rows: &[f64], taps: usize, per: u64) -> f64 {
    let sums: Vec<f64> = rows.chunks_exact(taps).map(|r| r.iter().sum()).collect();
    let mut total = 0.0;
    for w in sums.windows(4) {
        // The Lagrange weights for nodes -1, 0, 1, 2 integrate over [0, 1] to these.
        total += (13.0 * (w[1] + w[2]) - (w[0] + w[3])) / 24.0;
    }
    total / per as f64
}

/// Weights of the 4-point Lagrange interpolant through nodes -1, 0, 1, 2 at `t` in (0, 1).
fn lagrange(t: f64) -> [f64; 4] {
    let (a, b, c, d) = (t + 1.0, t, t - 1.0, t - 2.0);
    [
        -(b * c * d) / 6.0,
        (a * c * d) / 2.0,
        -(a * b * d) / 2.0,
        (a * b * c) / 6.0,
    ]
}

/// Row `r` holds the coefficients for fractional position `pos(r)`: tap `k` multiplies the
/// sample `k - taps / 2 + 1` places from the position's integer part.
fn rows(kernel: &Kernel, taps: usize, count: usize, pos: impl Fn(usize) -> f64) -> Vec<f64> {
    let top = (taps / 2) as f64 - 1.0;
    let mut table = Vec::with_capacity(count * taps);
    for r in 0..count {
        let f = pos(r);
        table.extend((0..taps).map(|k| kernel.at(f + top - k as f64)));
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lagrange_weights_sum_to_one_and_reproduce_cubics() {
        for t in [0.1, 0.25, 0.5, 0.9] {
            let w = lagrange(t);
            assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-15);
            let cubic = |x: f64| 2.0 * x * x * x - x * x + 3.0;
            let got: f64 = w
                .iter()
                .zip([-1.0, 0.0, 1.0, 2.0])
                .map(|(w, x)| w * cubic(x))
                .sum();
            assert!((got - cubic(t)).abs() < 1e-14);
        }
    }

    #[test]
    fn integral_is_exact_for_a_constant_kernel() {
        // Two taps of constant 1 at every row: the kernel integrates to 2 over a sample.
        let rows = vec![1.0; 2 * (8 + 3)];
        assert!((integral(&rows, 2, 8) - 2.0).abs() < 1e-15);
    }
}
