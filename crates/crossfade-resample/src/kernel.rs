//! The hot loops. Each is a standalone `#[inline(never)]` function over zipped fixed-size
//! chunks with no indexing inside, so it vectorises however the crate is split into codegen
//! units. The order of every sum is written out: lanes are independent accumulators combined
//! in a fixed tree, so vectorisation cannot change a result (Rust never reassociates or fuses
//! float operations on its own, and nothing here asks it to).

use crate::Sample;

/// Accumulator lanes; [`crate::design::TAP_MULTIPLE`] is a multiple of it.
const LANES: usize = 16;

/// `sum(a[k] * b[k])` over equal lengths that are a multiple of [`LANES`].
#[inline(never)]
pub(crate) fn dot<S: Sample>(a: &[S], b: &[S]) -> S {
    let (a, _) = a.as_chunks::<LANES>();
    let (b, _) = b.as_chunks::<LANES>();
    let mut acc = [S::ZERO; LANES];
    for (x, y) in a.iter().zip(b) {
        for ((s, &p), &q) in acc.iter_mut().zip(x).zip(y) {
            *s = *s + p * q;
        }
    }
    let [
        a0,
        a1,
        a2,
        a3,
        a4,
        a5,
        a6,
        a7,
        a8,
        a9,
        a10,
        a11,
        a12,
        a13,
        a14,
        a15,
    ] = acc;
    (((a0 + a8) + (a4 + a12)) + ((a2 + a10) + (a6 + a14)))
        + (((a1 + a9) + (a5 + a13)) + ((a3 + a11) + (a7 + a15)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_sums_every_product() {
        let a: Vec<f64> = (0..48).map(f64::from).collect();
        let b = vec![2.0; 48];
        assert_eq!(dot(&a, &b), 2.0 * (47.0 * 48.0 / 2.0));
        assert_eq!(dot::<f64>(&[], &[]), 0.0);
    }
}
