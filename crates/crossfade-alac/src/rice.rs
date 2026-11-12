//! ALAC's adaptive Golomb-Rice coder for prediction residuals: the Rice parameter follows a
//! running mean of recent magnitudes, and when the mean falls low enough a run of zeros is sent
//! as one count. Unsigned 32-bit arithmetic wraps, as in the reference coder, so hostile values
//! cannot panic.

use crate::bits::Bits;
use crate::{Error, Result};

pub(crate) const QBSHIFT: u32 = 9;
pub(crate) const QB: u32 = 1 << QBSHIFT;
pub(crate) const MMULSHIFT: u32 = 2;
pub(crate) const MDENSHIFT: u32 = QBSHIFT - MMULSHIFT - 1;
pub(crate) const MOFF: u32 = 1 << (MDENSHIFT - 2);
pub(crate) const BITOFF: u32 = 24;
/// A quotient this long is an escape: the value follows in full.
const MAX_PREFIX: u32 = 9;
/// Magnitudes above this pin the running mean.
const MEAN_CLAMP: u32 = 0xFFFF;
/// Bits of an escaped zero-run length.
const RUN_ESCAPE_BITS: u32 = 16;

/// `floor(log2(x + 3))`.
pub(crate) fn lg3a(x: u32) -> u32 {
    31 - x.wrapping_add(3).leading_zeros()
}

/// The coder's tuning for one channel of one packet.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Params {
    /// Initial running mean (`mb` in the config).
    pub(crate) mb: u32,
    /// How fast the mean follows (`pb * pb_factor / 4`).
    pub(crate) pb: u32,
    /// Largest Rice parameter (`kb`), 1 to 31.
    pub(crate) kb: u32,
}

/// One value: unary quotient, then the remainder in `k` bits, or `k - 1` when it is zero.
#[inline]
fn rice(bits: &mut Bits<'_>, m: u32, k: u32, escape_bits: u32) -> Result<u32> {
    let word = bits.peek64();
    let prefix = (!word).leading_zeros();
    if prefix >= MAX_PREFIX {
        bits.skip(MAX_PREFIX as usize)?;
        // A side channel at 32 bits has 33; the reference keeps the low 32.
        return Ok(bits.read_wide(escape_bits)? as u32);
    }
    let after = prefix + 1;
    // `after + k` is at most 9 + 31 bits, inside the 57 a peek holds.
    let v = ((word << after) >> (64 - k)) as u32;
    let q = prefix.wrapping_mul(m);
    if v < 2 {
        bits.skip((after + k - 1) as usize)?;
        Ok(q)
    } else {
        bits.skip((after + k) as usize)?;
        Ok(q.wrapping_add(v - 1))
    }
}

/// Decodes `out.len()` residuals of up to `chan_bits` bits.
pub(crate) fn decode(
    bits: &mut Bits<'_>,
    params: Params,
    chan_bits: u32,
    out: &mut [i32],
) -> Result<()> {
    let Params { mut mb, pb, kb } = params;
    let wb = (1u32 << kb) - 1;
    let mut zmode = 0u32;
    let mut c = 0;
    while c < out.len() {
        let k = lg3a(mb >> QBSHIFT).min(kb);
        let m = (1u32 << k) - 1;
        let n = rice(bits, m, k, chan_bits)?;
        let nd = n.wrapping_add(zmode);
        // The low bit is the sign: 0, -1, 1, -2, 2, ...
        let magnitude = (nd.wrapping_add(1) >> 1) as i32;
        out[c] = if nd & 1 != 0 {
            magnitude.wrapping_neg()
        } else {
            magnitude
        };
        c += 1;
        mb = pb
            .wrapping_mul(nd)
            .wrapping_add(mb)
            .wrapping_sub(pb.wrapping_mul(mb) >> QBSHIFT);
        if n > MEAN_CLAMP {
            mb = MEAN_CLAMP;
        }
        zmode = 0;
        if (mb << MMULSHIFT) < QB && c < out.len() {
            zmode = 1;
            // `mb` is below 128 here unless the shift above wrapped, which only a mean the
            // clamp keeps out could do; such a stream is broken.
            let k = (mb.leading_zeros() + ((mb + MOFF) >> MDENSHIFT))
                .checked_sub(BITOFF)
                .filter(|k| (1..32).contains(k))
                .ok_or(Error::Malformed("zero run parameter out of range"))?;
            let mz = ((1u32 << k) - 1) & wb;
            let run = rice(bits, mz, k, RUN_ESCAPE_BITS)? as usize;
            let zeros = out
                .get_mut(c..c.saturating_add(run))
                .ok_or(Error::Malformed("run of zeros past the end of the frame"))?;
            zeros.fill(0);
            c += run;
            if run >= 0xFFFF {
                zmode = 0;
            }
            mb = 0;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{BitWriter, put_residuals};
    use proptest::prelude::*;

    const AFCONVERT: Params = Params {
        mb: 10,
        pb: 40,
        kb: 14,
    };

    fn decoded(bytes: &[u8], params: Params, chan_bits: u32, n: usize) -> Result<(Vec<i32>, usize)> {
        let mut bits = Bits::new(bytes);
        let mut out = vec![7; n];
        decode(&mut bits, params, chan_bits, &mut out)?;
        Ok((out, bits.position()))
    }

    #[test]
    fn lg3a_is_the_log_of_x_plus_three() {
        for (x, want) in [(0, 1), (1, 2), (4, 2), (5, 3), (13, 4), (u32::MAX - 3, 31)] {
            assert_eq!(lg3a(x), want, "{x}");
        }
    }

    #[test]
    fn a_vector_worked_by_hand() {
        // Mean 10: k = lg3a(0) = 1, m = 1. Value 5 codes as 10: a quotient of 10 escapes, nine
        // ones and 10 in 16 bits. The mean becomes 40 * 10 + 10 - 0 = 410: still k = 1, and -1
        // codes as 1, the quotient 1 in unary.
        let mut w = BitWriter::default();
        w.put(9, 0x1FF);
        w.put(16, 10);
        w.put(2, 0b10);
        let (out, used) = decoded(&w.bytes, AFCONVERT, 16, 2).unwrap();
        assert_eq!(out, [5, -1]);
        assert_eq!(used, 27);
    }

    #[test]
    fn zero_runs_after_a_small_mean() {
        // Mean 0 and pb 40: after a zero the mean stays 0, under QB / 4, so a run count follows.
        // k = 32 - 24 + 0 = 8, m = 255 & wb: a run of 3 is quotient 0, then 4 in 8 bits.
        let p = Params {
            mb: 0,
            pb: 40,
            kb: 14,
        };
        let mut w = BitWriter::default();
        w.put(1, 0); // the first value, 0 (k = 1: just the unary zero)
        w.put(9, 0b0_0000_0100); // the run: 3 zeros
        // Then the next value with zmode set: 2 codes as 4 - 1 = 3, k = 1: three ones and a zero.
        w.put(4, 0b1110);
        let (out, used) = decoded(&w.bytes, p, 16, 5).unwrap();
        assert_eq!(out, [0, 0, 0, 0, 2]);
        assert_eq!(used, 14);
    }

    #[test]
    fn a_run_past_the_frame_or_a_short_packet_is_an_error() {
        let p = Params {
            mb: 0,
            pb: 40,
            kb: 14,
        };
        let mut w = BitWriter::default();
        w.put(1, 0);
        w.put(9, 0b0_0000_0110); // 5 zeros after the first value: one too many for 5 values
        w.put(8, 0);
        assert_eq!(
            decoded(&w.bytes, p, 16, 5),
            Err(Error::Malformed("run of zeros past the end of the frame"))
        );
        assert_eq!(decoded(&[0xFF, 0xFF], AFCONVERT, 16, 1), Err(Error::UnexpectedEnd));
        assert_eq!(decoded(&[], AFCONVERT, 16, 1), Err(Error::UnexpectedEnd));
    }

    fn residuals() -> impl Strategy<Value = (Vec<i32>, u32)> {
        (1u32..=33).prop_flat_map(|bits| {
            let max = if bits >= 32 {
                i32::MAX
            } else {
                (1i32 << (bits - 1)) - 1
            };
            let min = if bits >= 32 { i32::MIN + 1 } else { -max - 1 };
            let value = prop_oneof![
                4 => -3i32..=3,
                2 => Just(0),
                1 => -300i32..=300,
                1 => min..=max,
            ];
            let in_range = prop::collection::vec(value.prop_map(move |v| v.clamp(min, max)), 1..600);
            (in_range, Just(bits))
        })
    }

    proptest! {
        #[test]
        fn decodes_what_the_coder_writes(
            (values, chan_bits) in residuals(),
            mb in 0u32..=255,
            pb in 0u32..=446,
            kb in 1u32..=31,
        ) {
            let p = Params { mb, pb, kb };
            let mut w = BitWriter::default();
            put_residuals(&mut w, &values, mb, pb, kb, chan_bits);
            let (out, used) = decoded(&w.bytes, p, chan_bits, values.len()).unwrap();
            prop_assert_eq!(out, values);
            prop_assert_eq!(used, w.len());
        }

        #[test]
        fn hostile_bits_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..64),
                                    mb in any::<u8>(), pb in 0u32..=446, kb in 1u32..=31,
                                    chan_bits in 1u32..=33, n in 1usize..300) {
            let _ = decoded(&bytes, Params { mb: u32::from(mb), pb, kb }, chan_bits, n);
        }
    }
}
