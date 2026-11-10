//! Independent little-endian PCM codec, used as an oracle for the engine's
//! own (faster) converters. Kept simple on purpose.

use crossfade_core::SampleFormat;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcmError {
    /// Byte length is not a whole number of samples.
    PartialSample { len: usize, sample_size: usize },
}

impl fmt::Display for PcmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PartialSample { len, sample_size } => {
                write!(f, "{len} bytes is not a multiple of {sample_size}")
            }
        }
    }
}

impl std::error::Error for PcmError {}

pub fn encode_le(_samples: &[f64], _format: SampleFormat) -> Vec<u8> {
    Vec::new()
}

pub fn decode_le(_bytes: &[u8], _format: SampleFormat) -> Result<Vec<f64>, PcmError> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use SampleFormat::*;

    const PROBE: [f64; 6] = [0.0, 1.0, -1.0, 0.5, 1.5, -1.5];

    #[test]
    fn u8_is_offset_binary() {
        assert_eq!(encode_le(&PROBE, U8), [0x80, 0xFF, 0x00, 0xC0, 0xFF, 0x00]);
    }

    #[test]
    fn s16_known_bytes() {
        assert_eq!(
            encode_le(&PROBE, S16),
            [0, 0, 0xFF, 0x7F, 0, 0x80, 0, 0x40, 0xFF, 0x7F, 0, 0x80]
        );
    }

    #[test]
    fn s24_packs_three_bytes() {
        assert_eq!(
            encode_le(&PROBE, S24),
            [
                0, 0, 0, 0xFF, 0xFF, 0x7F, 0, 0, 0x80, 0, 0, 0x40, 0xFF, 0xFF, 0x7F, 0, 0, 0x80
            ]
        );
    }

    #[test]
    fn s32_known_bytes() {
        assert_eq!(
            encode_le(&PROBE, S32),
            [
                0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0x7F, 0, 0, 0, 0x80, 0, 0, 0, 0x40, 0xFF, 0xFF, 0xFF,
                0x7F, 0, 0, 0, 0x80
            ]
        );
    }

    #[test]
    fn floats_store_values_as_is() {
        assert_eq!(
            encode_le(&[0.5, 1.5], F32),
            [0, 0, 0, 0x3F, 0, 0, 0xC0, 0x3F]
        );
        assert_eq!(
            encode_le(&[1.0, -2.0], F64),
            [0, 0, 0, 0, 0, 0, 0xF0, 0x3F, 0, 0, 0, 0, 0, 0, 0, 0xC0]
        );
    }

    #[test]
    fn rounds_half_away_from_zero() {
        // 0.5 LSB of S16 is 2^-16.
        let half = 1.0 / 65_536.0;
        assert_eq!(encode_le(&[half, -half], S16), [1, 0, 0xFF, 0xFF]);
        let under = half * 0.99;
        assert_eq!(encode_le(&[under, -under], S16), [0, 0, 0, 0]);
    }

    #[test]
    fn round_trip_within_half_lsb() {
        let xs: Vec<f64> = (0..1000).map(|i| f64::from(i) / 500.0 - 1.0).collect();
        for (fmt, bits) in [(U8, 8), (S16, 16), (S24, 24), (S32, 32)] {
            let lsb = 1.0 / f64::from(1u32 << (bits - 1));
            let back = decode_le(&encode_le(&xs, fmt), fmt).unwrap();
            assert_eq!(back.len(), xs.len(), "{fmt:?}");
            for (x, y) in xs.iter().zip(&back) {
                // 1.0 is not representable and clamps one LSB down.
                let want = x.min(1.0 - lsb);
                assert!((want - y).abs() <= lsb / 2.0 + 1e-15, "{fmt:?} {x} {y}");
            }
        }
    }

    #[test]
    fn round_trip_exact_for_floats() {
        let xs = [0.1, -0.3, 1.0 / 3.0, 7.5];
        assert_eq!(decode_le(&encode_le(&xs, F64), F64).unwrap(), xs);
        let back = decode_le(&encode_le(&[0.5, -0.25], F32), F32).unwrap();
        assert_eq!(back, [0.5, -0.25]);
    }

    #[test]
    fn decode_known_values() {
        assert_eq!(
            decode_le(&[0x80, 0xFF, 0], U8).unwrap(),
            [0.0, 127.0 / 128.0, -1.0]
        );
        assert_eq!(decode_le(&[0, 0x80], S16).unwrap(), [-1.0]);
    }

    #[test]
    fn partial_sample_is_an_error() {
        for (fmt, n) in [(S16, 2), (S24, 3), (S32, 4), (F32, 4), (F64, 8)] {
            let err = decode_le(&vec![0; n + 1], fmt).unwrap_err();
            assert_eq!(
                err,
                PcmError::PartialSample {
                    len: n + 1,
                    sample_size: n
                }
            );
        }
        assert!(decode_le(&[], S16).unwrap().is_empty());
    }
}
