//! The inverse complex FFT of 480, 240, 120 or 60 points the IMDCT runs on (`celt/kiss_fft.c`,
//! `opus_ifft`, float build): mixed radix 4, 2, 3, 5 over the mode's twiddles, unscaled.

use super::tables::{BITREV60, BITREV120, BITREV240, BITREV480, FFT_FACTORS, FFT_TWIDDLES};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct Cpx {
    pub(super) r: f32,
    pub(super) i: f32,
}

/// `C_MULC`: `a` times the conjugate of the twiddle `b`.
#[inline(always)]
fn mulc(a: Cpx, b: (f32, f32)) -> Cpx {
    Cpx {
        r: a.r * b.0 + a.i * b.1,
        i: a.i * b.0 - a.r * b.1,
    }
}

#[inline(always)]
fn add(a: Cpx, b: Cpx) -> Cpx {
    Cpx {
        r: a.r + b.r,
        i: a.i + b.i,
    }
}

#[inline(always)]
fn sub(a: Cpx, b: Cpx) -> Cpx {
    Cpx {
        r: a.r - b.r,
        i: a.i - b.i,
    }
}

/// Points of the FFT at `shift` (0 to 3).
pub(super) fn size(shift: usize) -> usize {
    480 >> shift
}

fn bfly2(f: &mut [Cpx], fstride: usize, m: usize, n: usize, mm: usize) {
    for i in 0..n {
        let base = i * mm;
        for j in 0..m {
            let t = mulc(f[base + m + j], FFT_TWIDDLES[j * fstride]);
            f[base + m + j] = sub(f[base + j], t);
            f[base + j] = add(f[base + j], t);
        }
    }
}

fn bfly3(f: &mut [Cpx], fstride: usize, m: usize, n: usize, mm: usize) {
    let epi3 = FFT_TWIDDLES[fstride * m];
    for i in 0..n {
        let base = i * mm;
        for k in 0..m {
            let at = base + k;
            let s1 = mulc(f[at + m], FFT_TWIDDLES[k * fstride]);
            let s2 = mulc(f[at + 2 * m], FFT_TWIDDLES[2 * k * fstride]);
            let s3 = add(s1, s2);
            let mut s0 = sub(s1, s2);
            let f0 = f[at];
            let mut fm = Cpx {
                r: f0.r - s3.r * 0.5,
                i: f0.i - s3.i * 0.5,
            };
            s0.r *= -epi3.1;
            s0.i *= -epi3.1;
            f[at] = add(f0, s3);
            f[at + 2 * m] = Cpx {
                r: fm.r + s0.i,
                i: fm.i - s0.r,
            };
            fm.r -= s0.i;
            fm.i += s0.r;
            f[at + m] = fm;
        }
    }
}

fn bfly4(f: &mut [Cpx], fstride: usize, m: usize, n: usize, mm: usize) {
    for i in 0..n {
        let base = i * mm;
        for j in 0..m {
            let at = base + j;
            let s0 = mulc(f[at + m], FFT_TWIDDLES[j * fstride]);
            let s1 = mulc(f[at + 2 * m], FFT_TWIDDLES[2 * j * fstride]);
            let s2 = mulc(f[at + 3 * m], FFT_TWIDDLES[3 * j * fstride]);
            let f0 = f[at];
            let s5 = sub(f0, s1);
            let f0 = add(f0, s1);
            let s3 = add(s0, s2);
            let s4 = sub(s0, s2);
            f[at + 2 * m] = sub(f0, s3);
            f[at] = add(f0, s3);
            f[at + m] = Cpx {
                r: s5.r - s4.i,
                i: s5.i + s4.r,
            };
            f[at + 3 * m] = Cpx {
                r: s5.r + s4.i,
                i: s5.i - s4.r,
            };
        }
    }
}

fn bfly5(f: &mut [Cpx], fstride: usize, m: usize, n: usize, mm: usize) {
    let ya = FFT_TWIDDLES[fstride * m];
    let yb = FFT_TWIDDLES[fstride * 2 * m];
    for i in 0..n {
        let base = i * mm;
        for u in 0..m {
            let a0 = base + u;
            let (a1, a2, a3, a4) = (a0 + m, a0 + 2 * m, a0 + 3 * m, a0 + 4 * m);
            let s0 = f[a0];
            let s1 = mulc(f[a1], FFT_TWIDDLES[u * fstride]);
            let s2 = mulc(f[a2], FFT_TWIDDLES[2 * u * fstride]);
            let s3 = mulc(f[a3], FFT_TWIDDLES[3 * u * fstride]);
            let s4 = mulc(f[a4], FFT_TWIDDLES[4 * u * fstride]);
            let s7 = add(s1, s4);
            let s10 = sub(s1, s4);
            let s8 = add(s2, s3);
            let s9 = sub(s2, s3);
            f[a0] = Cpx {
                r: s0.r + (s7.r + s8.r),
                i: s0.i + (s7.i + s8.i),
            };
            let s5 = Cpx {
                r: s0.r + s7.r * ya.0 + s8.r * yb.0,
                i: s0.i + s7.i * ya.0 + s8.i * yb.0,
            };
            let s6 = Cpx {
                r: -(s10.i * ya.1) - s9.i * yb.1,
                i: s10.r * ya.1 + s9.r * yb.1,
            };
            f[a1] = sub(s5, s6);
            f[a4] = add(s5, s6);
            let s11 = Cpx {
                r: s0.r + s7.r * yb.0 + s8.r * ya.0,
                i: s0.i + s7.i * yb.0 + s8.i * ya.0,
            };
            let s12 = Cpx {
                r: s10.i * yb.1 - s9.i * ya.1,
                i: -(s10.r * yb.1) + s9.r * ya.1,
            };
            f[a2] = add(s11, s12);
            f[a3] = sub(s11, s12);
        }
    }
}

/// `opus_ifft` of `size(shift)` points: `fout[k] = sum fin[n] e^(2 pi i n k / size)`.
pub(super) fn ifft(shift: usize, fin: &[Cpx], fout: &mut [Cpx]) {
    let bitrev: &[u16] = match shift {
        0 => &BITREV480,
        1 => &BITREV240,
        2 => &BITREV120,
        _ => &BITREV60,
    };
    for (&x, &to) in fin.iter().zip(bitrev) {
        fout[usize::from(to)] = x;
    }
    let factors = FFT_FACTORS[shift.min(3)];
    let mut fstride = [1usize; 6];
    for (l, &(p, _)) in factors.iter().enumerate() {
        fstride[l + 1] = fstride[l] * p;
    }
    for i in (0..factors.len()).rev() {
        let (p, m) = factors[i];
        let mm = if i == 0 { 1 } else { factors[i - 1].1 };
        let tw = fstride[i] << shift;
        match p {
            2 => bfly2(fout, tw, m, fstride[i], mm),
            3 => bfly3(fout, tw, m, fstride[i], mm),
            4 => bfly4(fout, tw, m, fstride[i], mm),
            _ => bfly5(fout, tw, m, fstride[i], mm),
        }
    }
}

#[cfg(test)]
#[path = "fft_tests.rs"]
mod tests;
