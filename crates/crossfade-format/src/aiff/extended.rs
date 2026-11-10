//! The 80-bit IEEE 754 extended float AIFF stores its sample rate in.

use crossfade_core::Error;

/// A whole number of hertz in `1..=u32::MAX`.
pub(super) fn decode(bytes: &[u8; 10]) -> Result<u32, Error> {
    let sign_exp = u16::from_be_bytes([bytes[0], bytes[1]]);
    let mut m = [0u8; 8];
    m.copy_from_slice(&bytes[2..]);
    let mantissa = u64::from_be_bytes(m);
    let exp = sign_exp & 0x7FFF;
    let malformed = |what: &str| Error::Malformed {
        what: format!("sample rate is {what}"),
    };
    if exp == 0x7FFF {
        return Err(malformed("infinite or not a number"));
    }
    if mantissa == 0 {
        return Err(malformed("zero"));
    }
    if exp == 0 {
        return Err(malformed("a denormal"));
    }
    if sign_exp & 0x8000 != 0 {
        return Err(malformed("negative"));
    }
    let unsupported = |what: &str| Error::Unsupported {
        what: format!("sample rate is {what}"),
    };
    // value = mantissa * 2^shift, the mantissa being a plain 64-bit integer.
    let shift = i32::from(exp) - 16383 - 63;
    let hz = if shift >= 0 {
        if shift > 32 {
            return Err(unsupported("above 4294967295 Hz"));
        }
        u128::from(mantissa) << shift
    } else {
        let down = shift.unsigned_abs();
        if down >= 64 || mantissa & ((1u64 << down) - 1) != 0 {
            return Err(unsupported("not a whole number of hertz"));
        }
        u128::from(mantissa >> down)
    };
    u32::try_from(hz).map_err(|_| unsupported("above 4294967295 Hz"))
}

pub(super) fn encode(hz: u32) -> [u8; 10] {
    let mut out = [0u8; 10];
    if hz == 0 {
        return out;
    }
    let top = 31 - hz.leading_zeros();
    let exp = 16383 + top as u16;
    out[..2].copy_from_slice(&exp.to_be_bytes());
    out[2..].copy_from_slice(&(u64::from(hz) << (63 - top)).to_be_bytes());
    out
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
