//! Comparison functions for byte and sample buffers.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LengthMismatch {
    pub a: usize,
    pub b: usize,
}

impl fmt::Display for LengthMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "length mismatch: {} vs {}", self.a, self.b)
    }
}

impl std::error::Error for LengthMismatch {}

pub fn bit_exact(a: &[u8], b: &[u8]) -> bool {
    a == b
}

/// Index of the first differing element, or the shorter length when one
/// input is a prefix of the other.
pub fn first_difference<T: PartialEq>(a: &[T], b: &[T]) -> Option<usize> {
    let common = a.len().min(b.len());
    match (0..common).find(|&i| a[i] != b[i]) {
        Some(i) => Some(i),
        None if a.len() == b.len() => None,
        None => Some(common),
    }
}

fn check_len(a: &[f64], b: &[f64]) -> Result<(), LengthMismatch> {
    if a.len() == b.len() {
        Ok(())
    } else {
        Err(LengthMismatch {
            a: a.len(),
            b: b.len(),
        })
    }
}

pub fn peak_abs_diff(a: &[f64], b: &[f64]) -> Result<f64, LengthMismatch> {
    check_len(a, b)?;
    Ok(a.iter().zip(b).fold(0.0, |m, (x, y)| m.max((x - y).abs())))
}

pub fn rms_diff(a: &[f64], b: &[f64]) -> Result<f64, LengthMismatch> {
    check_len(a, b)?;
    if a.is_empty() {
        return Ok(0.0);
    }
    let sum: f64 = a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum();
    Ok((sum / a.len() as f64).sqrt())
}

/// `10 * log10(signal power / error power)`. Zero error is `INFINITY`; a
/// silent reference with any error is `NEG_INFINITY`.
pub fn snr_db(reference: &[f64], test: &[f64]) -> Result<f64, LengthMismatch> {
    check_len(reference, test)?;
    let error: f64 = reference
        .iter()
        .zip(test)
        .map(|(r, t)| (r - t) * (r - t))
        .sum();
    if error == 0.0 {
        return Ok(f64::INFINITY);
    }
    let signal: f64 = reference.iter().map(|r| r * r).sum();
    if signal == 0.0 {
        return Ok(f64::NEG_INFINITY);
    }
    Ok(10.0 * libm::log10(signal / error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_exact_compares_bytes_and_length() {
        assert!(bit_exact(b"abc", b"abc"));
        assert!(!bit_exact(b"abc", b"abd"));
        assert!(!bit_exact(b"abc", b"ab"));
        assert!(bit_exact(b"", b""));
    }

    #[test]
    fn first_difference_finds_index_or_prefix_length() {
        assert_eq!(first_difference(&[1, 2, 3], &[1, 2, 3]), None);
        assert_eq!(first_difference(&[1, 2, 3], &[1, 9, 3]), Some(1));
        assert_eq!(first_difference(&[1, 2, 3], &[1, 2]), Some(2));
        assert_eq!(first_difference(&[1, 2], &[1, 2, 3]), Some(2));
        assert_eq!(first_difference::<u8>(&[], &[]), None);
        assert_eq!(
            first_difference(&[1.0, f64::NAN], &[1.0, f64::NAN]),
            Some(1)
        );
    }

    #[test]
    fn peak_and_rms_of_constant_offset() {
        let r = [1.0; 4];
        let t = [0.9; 4];
        assert!((peak_abs_diff(&r, &t).unwrap() - 0.1).abs() < 1e-12);
        assert!((rms_diff(&r, &t).unwrap() - 0.1).abs() < 1e-12);
    }

    #[test]
    fn peak_and_rms_differ_for_a_single_outlier() {
        let r = [0.0; 4];
        let t = [0.0, 0.0, 0.0, -2.0];
        assert_eq!(peak_abs_diff(&r, &t).unwrap(), 2.0);
        assert!((rms_diff(&r, &t).unwrap() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn snr_of_ten_percent_error_is_20_db() {
        let snr = snr_db(&[1.0; 8], &[0.9; 8]).unwrap();
        assert!((snr - 20.0).abs() < 1e-9, "{snr}");
    }

    #[test]
    fn snr_edge_cases() {
        let x = [0.5, -0.5];
        assert_eq!(snr_db(&x, &x).unwrap(), f64::INFINITY);
        assert_eq!(snr_db(&[0.0; 2], &[0.1; 2]).unwrap(), f64::NEG_INFINITY);
        assert_eq!(snr_db(&[0.0; 2], &[0.0; 2]).unwrap(), f64::INFINITY);
    }

    #[test]
    fn length_mismatch_is_an_error() {
        let err = LengthMismatch { a: 2, b: 3 };
        assert_eq!(peak_abs_diff(&[0.0; 2], &[0.0; 3]), Err(err));
        assert_eq!(rms_diff(&[0.0; 2], &[0.0; 3]), Err(err));
        assert_eq!(snr_db(&[0.0; 2], &[0.0; 3]), Err(err));
    }

    #[test]
    fn empty_inputs_are_identical() {
        assert_eq!(peak_abs_diff(&[], &[]), Ok(0.0));
        assert_eq!(rms_diff(&[], &[]), Ok(0.0));
        assert_eq!(snr_db(&[], &[]), Ok(f64::INFINITY));
    }
}
