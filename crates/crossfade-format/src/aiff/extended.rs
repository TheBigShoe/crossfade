//! The 80-bit IEEE 754 extended float AIFF stores its sample rate in.

use crossfade_core::Error;

/// A whole number of hertz in `1..=u32::MAX`.
pub(super) fn decode(_bytes: &[u8; 10]) -> Result<u32, Error> {
    todo!()
}

pub(super) fn encode(_hz: u32) -> [u8; 10] {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn ext(sign_exp: u16, mantissa: u64) -> [u8; 10] {
        let mut b = [0u8; 10];
        b[..2].copy_from_slice(&sign_exp.to_be_bytes());
        b[2..].copy_from_slice(&mantissa.to_be_bytes());
        b
    }

    fn kind(r: Result<u32, Error>) -> &'static str {
        match r {
            Ok(_) => "ok",
            Err(Error::Malformed { .. }) => "malformed",
            Err(Error::Unsupported { .. }) => "unsupported",
            Err(e) => panic!("{e:?}"),
        }
    }

    #[test]
    fn the_classic_rates_have_the_bytes_every_aiff_writer_emits() {
        for (hz, bytes) in [
            (44_100, [0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]),
            (48_000, [0x40, 0x0E, 0xBB, 0x80, 0, 0, 0, 0, 0, 0]),
            (22_050, [0x40, 0x0D, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]),
            (8_000, [0x40, 0x0B, 0xFA, 0, 0, 0, 0, 0, 0, 0]),
            (96_000, [0x40, 0x0F, 0xBB, 0x80, 0, 0, 0, 0, 0, 0]),
            (1, [0x3F, 0xFF, 0x80, 0, 0, 0, 0, 0, 0, 0]),
            (
                4_294_967_295,
                [0x40, 0x1E, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0],
            ),
        ] {
            assert_eq!(encode(hz), bytes, "{hz}");
            assert_eq!(decode(&bytes), Ok(hz), "{hz}");
        }
    }

    #[test]
    fn an_unnormalised_whole_number_still_decodes() {
        // 44100 with the integer bit not at the top: 44100 * 2^0 as a plain integer mantissa.
        assert_eq!(decode(&ext(0x3FFF + 63, 44_100)), Ok(44_100));
    }

    #[test]
    fn fractional_and_oversized_rates_are_unsupported() {
        // 44100.5 = 0xAC44 8000 >> 1 ... mantissa carries one fractional bit.
        let half = (88_201u64) << 47;
        assert_eq!(kind(decode(&ext(0x400E, half))), "unsupported");
        // 0.5, 2^32, 2^100.
        assert_eq!(kind(decode(&ext(0x3FFE, 1 << 63))), "unsupported");
        assert_eq!(kind(decode(&ext(0x401F, 1 << 63))), "unsupported");
        assert_eq!(kind(decode(&ext(0x3FFF + 100, 1 << 63))), "unsupported");
    }

    #[test]
    fn zero_negative_infinite_nan_and_denormal_are_malformed() {
        for (name, b) in [
            ("zero", ext(0, 0)),
            ("negative zero", ext(0x8000, 0)),
            ("negative", ext(0xC00E, 0xAC44 << 48)),
            ("infinity", ext(0x7FFF, 1 << 63)),
            ("nan", ext(0x7FFF, 0xC000_0000_0000_0000)),
            ("denormal", ext(0, 1)),
            ("pseudo-zero", ext(0x400E, 0)),
        ] {
            assert_eq!(kind(decode(&b)), "malformed", "{name}");
        }
    }

    proptest! {
        #[test]
        fn every_u32_round_trips(hz in 1u32..) {
            prop_assert_eq!(decode(&encode(hz)), Ok(hz));
        }
    }
}
