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
    let a = acc;
    (((a[0] + a[8]) + (a[4] + a[12])) + ((a[2] + a[10]) + (a[6] + a[14])))
        + (((a[1] + a[9]) + (a[5] + a[13])) + ((a[3] + a[11]) + (a[7] + a[15])))
}

/// `acc[k] += x * coefs[k]`: one input sample spread over the outputs it reaches.
#[inline(never)]
pub(crate) fn axpy<S: Sample>(acc: &mut [S], x: S, coefs: &[S]) {
    for (a, &c) in acc.iter_mut().zip(coefs) {
        *a = *a + x * c;
    }
}

/// `out[k] = w0 r0[k] + w1 r1[k] + w2 r2[k] + w3 r3[k]`, summed left to right.
#[inline(never)]
pub(crate) fn blend4<S: Sample>(out: &mut [S], w: [S; 4], rows: [&[S]; 4]) {
    let [w0, w1, w2, w3] = w;
    let [r0, r1, r2, r3] = rows;
    for ((((o, &a), &b), &c), &d) in out.iter_mut().zip(r0).zip(r1).zip(r2).zip(r3) {
        *o = w0 * a + w1 * b + w2 * c + w3 * d;
    }
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

    #[test]
    fn axpy_and_blend4_combine_rows() {
        let mut acc = vec![1.0, 2.0];
        axpy(&mut acc, 2.0, &[0.5, -1.0]);
        assert_eq!(acc, [2.0, 0.0]);
        let mut out = vec![0.0; 2];
        blend4(
            &mut out,
            [1.0, 2.0, 3.0, 4.0],
            [&[1.0, 0.0], &[1.0, 1.0], &[1.0, 2.0], &[1.0, 3.0]],
        );
        assert_eq!(out, [10.0, 20.0]);
    }
}
