//! Test-only writers: a bit writer, and an encoder for the adaptive Rice coder and the packet
//! syntax, written from the format description so the decoder can be checked on bitstreams no
//! encoder at hand produces (the "prediction on prediction" mode, DSE and FIL elements, odd
//! shifts).

use crate::rice::{BITOFF, MDENSHIFT, MMULSHIFT, MOFF, QB, QBSHIFT, lg3a};

#[derive(Default)]
pub(crate) struct BitWriter {
    pub(crate) bytes: Vec<u8>,
    bits: usize,
}

impl BitWriter {
    /// The low `n` bits of `v`, high first; past 64 bits, zeros (or ones for `u64::MAX`).
    pub(crate) fn put(&mut self, n: u32, v: u64) {
        for i in (0..n).rev() {
            let bit = if i < 64 { (v >> i) & 1 } else { v >> 63 };
            if self.bits.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if bit != 0 {
                *self.bytes.last_mut().unwrap() |= 0x80 >> (self.bits % 8);
            }
            self.bits += 1;
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.bits
    }

    pub(crate) fn align(&mut self) {
        self.bits = self.bits.next_multiple_of(8);
    }
}

/// One Rice-coded value as the decoder reads it: `n = q * m + r` with `m = 2^k - 1`, the
/// quotient in unary (ones, then a zero), then `k - 1` zero bits for `r == 0` or `r + 1` in `k`
/// bits; a quotient of 9 or more is nine ones and `n` in `escape` bits.
fn put_rice(w: &mut BitWriter, n: u32, m: u32, k: u32, escape: u32) {
    let q = n.checked_div(m).unwrap_or(9);
    if q >= 9 {
        w.put(9, 0x1FF);
        w.put(escape, u64::from(n));
        return;
    }
    let r = n - q * m;
    w.put(q + 1, ((1u64 << q) - 1) << 1);
    if r == 0 {
        w.put(k - 1, 0);
    } else {
        w.put(k, u64::from(r + 1));
    }
}

/// The adaptive Rice coder's encoding side for residuals of at most `chan_bits` bits.
pub(crate) fn put_residuals(
    w: &mut BitWriter,
    values: &[i32],
    mb0: u32,
    pb: u32,
    kb: u32,
    chan_bits: u32,
) {
    let wb = (1u32 << kb) - 1;
    let mut mb = mb0;
    let mut zmode = 0u32;
    let mut c = 0;
    while c < values.len() {
        let k = lg3a(mb >> QBSHIFT).min(kb);
        let m = (1u32 << k) - 1;
        let v = values[c];
        let nd = if v >= 0 {
            (v as u32).wrapping_mul(2)
        } else {
            (v.wrapping_neg() as u32).wrapping_mul(2).wrapping_sub(1)
        };
        let n = nd.wrapping_sub(zmode);
        put_rice(w, n, m, k, chan_bits);
        c += 1;
        mb = pb
            .wrapping_mul(n.wrapping_add(zmode))
            .wrapping_add(mb)
            .wrapping_sub(pb.wrapping_mul(mb) >> QBSHIFT);
        if n > 0xFFFF {
            mb = 0xFFFF;
        }
        zmode = 0;
        if (mb << MMULSHIFT) < QB && c < values.len() {
            zmode = 1;
            let k = mb.leading_zeros() - BITOFF + ((mb + MOFF) >> MDENSHIFT);
            let mz = ((1u32 << k) - 1) & wb;
            let run = values[c..]
                .iter()
                .take(0xFFFF)
                .take_while(|&&v| v == 0)
                .count() as u32;
            put_rice(w, run, mz, k, 16);
            c += run as usize;
            if run >= 0xFFFF {
                zmode = 0;
            }
            mb = 0;
        }
    }
}

/// The predictor's encoding side: residuals for `x`, adapting `coefs` the way the decoder
/// will. The order is `coefs.len()` (0 and 31 as in `predictor::unpredict`).
pub(crate) fn predict(x: &[i32], coefs: &mut [i16], chan_bits: u32, den_shift: u32) -> Vec<i32> {
    use crate::bits::sign_extend;
    let order = coefs.len();
    if x.is_empty() || order == 0 {
        return x.to_vec();
    }
    let mut r = vec![x[0]];
    let warm = if order == 31 {
        x.len() - 1
    } else {
        order.min(x.len() - 1)
    };
    for j in 1..=warm {
        r.push(sign_extend(x[j].wrapping_sub(x[j - 1]), chan_bits));
    }
    if order == 31 {
        return r;
    }
    let den_half = if den_shift == 0 {
        0
    } else {
        1i32 << (den_shift - 1)
    };
    for j in order + 1..x.len() {
        let top = x[j - order - 1];
        let mut sum = 0i32;
        for (k, c) in coefs.iter().enumerate() {
            sum = sum.wrapping_add(i32::from(*c).wrapping_mul(x[j - 1 - k].wrapping_sub(top)));
        }
        let predicted = sum.wrapping_add(den_half) >> den_shift;
        let del = sign_extend(x[j].wrapping_sub(top).wrapping_sub(predicted), chan_bits);
        r.push(del);
        let mut left = del;
        for k in (0..order).rev() {
            if del == 0 {
                break;
            }
            let dd = top.wrapping_sub(x[j - 1 - k]);
            let sgn = dd.signum();
            let step = (order - k) as i32;
            if del > 0 {
                coefs[k] = coefs[k].wrapping_sub(sgn as i16);
                left = left.wrapping_sub(step.wrapping_mul(sgn.wrapping_mul(dd) >> den_shift));
                if left <= 0 {
                    break;
                }
            } else {
                coefs[k] = coefs[k].wrapping_add(sgn as i16);
                left = left.wrapping_sub(step.wrapping_mul((-sgn).wrapping_mul(dd) >> den_shift));
                if left >= 0 {
                    break;
                }
            }
        }
    }
    r
}
