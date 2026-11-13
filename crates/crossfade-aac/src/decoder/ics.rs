//! One individual channel stream (14496-3 4.4.2.7, 4.6.2-4.6.3): ics_info, section data,
//! scalefactors, pulse and TNS data, spectral data, inverse quantisation and scaling.

use super::filterbank::EIGHT_SHORT;
use super::huffman::{Books, books};
use super::tables::{SWB_LONG, SWB_SHORT, TNS_MAX_ORDER_LONG, TNS_MAX_ORDER_SHORT};
use crate::Result;
use crate::bits::BitReader;
use crate::error::{malformed, unsupported};
use std::sync::OnceLock;

pub(crate) const ZERO_HCB: u8 = 0;
pub(crate) const ESC_HCB: u8 = 11;
pub(crate) const NOISE_HCB: u8 = 13;
pub(crate) const INTENSITY_HCB2: u8 = 14;
pub(crate) const INTENSITY_HCB: u8 = 15;

/// Bands per window at most (51 at 32 kHz), rounded up.
pub(crate) const MAX_SFB: usize = 64;

#[derive(Clone, Copy)]
pub(crate) struct IcsInfo {
    pub(crate) window_sequence: u8,
    pub(crate) window_shape: u8,
    pub(crate) max_sfb: usize,
    pub(crate) num_windows: usize,
    pub(crate) num_groups: usize,
    pub(crate) group_len: [usize; 8],
    /// Band offsets for this window size, with the closing 1024 or 128.
    pub(crate) swb: &'static [u16],
}

impl IcsInfo {
    pub(crate) fn is_short(&self) -> bool {
        self.window_sequence == EIGHT_SHORT
    }

    pub(crate) fn read(r: &mut BitReader<'_>, sampling_index: usize) -> Result<Self> {
        r.skip(1); // ics_reserved_bit
        let window_sequence = r.read(2) as u8;
        let window_shape = r.read(1) as u8;
        let mut info = Self {
            window_sequence,
            window_shape,
            max_sfb: 0,
            num_windows: 1,
            num_groups: 1,
            group_len: [1, 0, 0, 0, 0, 0, 0, 0],
            swb: SWB_LONG[sampling_index],
        };
        if window_sequence == EIGHT_SHORT {
            info.max_sfb = r.read(4) as usize;
            let grouping = r.read(7);
            info.num_windows = 8;
            info.swb = SWB_SHORT[sampling_index];
            for i in 0..7 {
                if grouping & (1 << (6 - i)) != 0 {
                    info.group_len[info.num_groups - 1] += 1;
                } else {
                    info.num_groups += 1;
                    info.group_len[info.num_groups - 1] = 1;
                }
            }
        } else {
            info.max_sfb = r.read(6) as usize;
            if r.bit() {
                return Err(unsupported("prediction (AAC Main or LTP) is not supported"));
            }
        }
        if info.max_sfb > info.swb.len() - 1 {
            return Err(malformed(format!(
                "max_sfb {} exceeds the {} bands of the window",
                info.max_sfb,
                info.swb.len() - 1
            )));
        }
        Ok(info)
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct TnsFilter {
    /// Bands the filter covers, counted down from the top of the previous one.
    pub(crate) length: usize,
    pub(crate) order: usize,
    pub(crate) direction: bool,
    /// Prediction coefficients a[1..=order] (a[0] = 1 implied).
    pub(crate) lpc: [f32; TNS_MAX_ORDER_LONG],
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Tns {
    pub(crate) present: bool,
    pub(crate) n_filt: [usize; 8],
    pub(crate) filt: [[TnsFilter; 3]; 8],
}

/// A decoded channel stream: its info, per-band codebooks and scalefactors, TNS, and the
/// dequantised spectrum (per window, 128 lines each when short).
pub(crate) struct Ics {
    pub(crate) info: IcsInfo,
    pub(crate) global_gain: u32,
    pub(crate) band_cb: [[u8; MAX_SFB]; 8],
    /// Scalefactor, intensity position or noise energy, by the band's codebook.
    pub(crate) sf: [[i32; MAX_SFB]; 8],
    pub(crate) tns: Tns,
    pub(crate) spec: Vec<f32>,
    quant: Vec<i32>,
}

impl Ics {
    pub(crate) fn new() -> Self {
        Self {
            info: IcsInfo {
                window_sequence: 0,
                window_shape: 0,
                max_sfb: 0,
                num_windows: 1,
                num_groups: 1,
                group_len: [1, 0, 0, 0, 0, 0, 0, 0],
                swb: SWB_LONG[4],
            },
            global_gain: 0,
            band_cb: [[0; MAX_SFB]; 8],
            sf: [[0; MAX_SFB]; 8],
            tns: Tns::default(),
            spec: vec![0.0; 1024],
            quant: vec![0; 1024],
        }
    }

    /// Reads individual_channel_stream(); `common` is the CPE's shared ics_info.
    pub(crate) fn read(
        &mut self,
        r: &mut BitReader<'_>,
        common: Option<IcsInfo>,
        sampling_index: usize,
    ) -> Result<()> {
        self.global_gain = r.read(8);
        self.info = match common {
            Some(info) => info,
            None => IcsInfo::read(r, sampling_index)?,
        };
        self.read_sections(r)?;
        self.read_scalefactors(r)?;
        let pulses = if r.bit() {
            if self.info.is_short() {
                return Err(malformed("pulse data in a short window"));
            }
            Some(read_pulses(r, &self.info)?)
        } else {
            None
        };
        self.tns.present = r.bit();
        if self.tns.present {
            self.read_tns(r, sampling_index)?;
        }
        if r.bit() {
            return Err(unsupported("gain control (AAC SSR) is not supported"));
        }
        match pulses {
            // Pulses change the quantised values before inverse quantisation.
            Some(p) => {
                self.read_spectrum(r)?;
                p.apply(&mut self.quant)?;
                self.dequantise()?;
            }
            None => self.read_spectrum_dequantised(r)?,
        }
        if r.overrun() {
            return Err(malformed("channel stream runs past the end of its block"));
        }
        Ok(())
    }

    /// Spectral data straight to the dequantised, de-interleaved spectrum: what
    /// [`read_spectrum`](Self::read_spectrum) and [`dequantise`](Self::dequantise) do, in one
    /// pass.
    fn read_spectrum_dequantised(&mut self, r: &mut BitReader<'_>) -> Result<()> {
        let books: &Books = books();
        let pow43 = pow43();
        let gains = gains();
        let info = self.info;
        self.spec.fill(0.0);
        let mut window = 0;
        for g in 0..info.num_groups {
            let glen = info.group_len[g];
            for b in 0..info.max_sfb {
                let cb = self.band_cb[g][b];
                if cb == ZERO_HCB || cb >= NOISE_HCB {
                    continue;
                }
                let gain = gains[self.sf[g][b] as usize];
                let (lo, hi) = (usize::from(info.swb[b]), usize::from(info.swb[b + 1]));
                let book = &books.spectral[usize::from(cb) - 1];
                let dim = book.shape.dim;
                for w in 0..glen {
                    let dst = (window + w) * 128 + lo;
                    for out in self.spec[dst..dst + hi - lo].chunks_exact_mut(dim) {
                        let sym = book.book.decode(r) as usize;
                        let vals = &book.values[sym][..dim];
                        let mut q = [0i32; 4];
                        if book.shape.signed {
                            for (q, &v) in q.iter_mut().zip(vals) {
                                *q = i32::from(v);
                            }
                        } else {
                            let nonzero = vals.iter().filter(|&&v| v != 0).count() as u32;
                            // The sign bits of the nonzero values, first value first.
                            let signs = r.read(nonzero);
                            let mut left = nonzero;
                            for (q, &v) in q.iter_mut().zip(vals) {
                                *q = i32::from(v);
                                if v != 0 {
                                    left -= 1;
                                    if (signs >> left) & 1 != 0 {
                                        *q = -*q;
                                    }
                                }
                            }
                            if cb == ESC_HCB {
                                for q in &mut q[..2] {
                                    if q.abs() == 16 {
                                        *q = q.signum() * escape(r)?;
                                    }
                                }
                            }
                        }
                        for (o, &q) in out.iter_mut().zip(&q) {
                            let m = pow43[q.unsigned_abs() as usize] * gain;
                            *o = if q < 0 { -m } else { m };
                        }
                    }
                }
                if r.overrun() {
                    return Err(malformed("spectral data runs past the end of the block"));
                }
            }
            window += glen;
        }
        Ok(())
    }

    fn read_sections(&mut self, r: &mut BitReader<'_>) -> Result<()> {
        let bits = if self.info.is_short() { 3 } else { 5 };
        let esc = (1 << bits) - 1;
        let max_sfb = self.info.max_sfb;
        for g in 0..self.info.num_groups {
            let mut k = 0;
            while k < max_sfb {
                let cb = r.read(4) as u8;
                if cb == 12 {
                    return Err(malformed("reserved codebook 12"));
                }
                let mut len = 0;
                loop {
                    let incr = r.read(bits);
                    len += incr as usize;
                    if incr != esc || r.overrun() {
                        break;
                    }
                }
                if len == 0 || k + len > max_sfb || r.overrun() {
                    return Err(malformed("section data does not cover the bands"));
                }
                self.band_cb[g][k..k + len].fill(cb);
                k += len;
            }
            self.band_cb[g][max_sfb..].fill(ZERO_HCB);
        }
        Ok(())
    }

    fn read_scalefactors(&mut self, r: &mut BitReader<'_>) -> Result<()> {
        let book = &books().scalefactor;
        let mut sf = self.global_gain as i32;
        let mut is_position = 0i32;
        let mut noise = self.global_gain as i32 - 90;
        let mut first_noise = true;
        for g in 0..self.info.num_groups {
            for b in 0..self.info.max_sfb {
                let v = match self.band_cb[g][b] {
                    ZERO_HCB => 0,
                    INTENSITY_HCB | INTENSITY_HCB2 => {
                        is_position += book.decode(r) as i32 - 60;
                        is_position
                    }
                    NOISE_HCB => {
                        if first_noise {
                            first_noise = false;
                            noise += r.read(9) as i32 - 256;
                        } else {
                            noise += book.decode(r) as i32 - 60;
                        }
                        noise
                    }
                    _ => {
                        sf += book.decode(r) as i32 - 60;
                        if !(0..=255).contains(&sf) {
                            return Err(malformed(format!("scalefactor {sf} out of range")));
                        }
                        sf
                    }
                };
                self.sf[g][b] = v;
            }
        }
        if r.overrun() {
            return Err(malformed("scalefactors run past the end of the block"));
        }
        Ok(())
    }

    fn read_tns(&mut self, r: &mut BitReader<'_>, sampling_index: usize) -> Result<()> {
        let short = self.info.is_short();
        let (filt_bits, len_bits, order_bits, max_order) = if short {
            (1, 4, 3, TNS_MAX_ORDER_SHORT)
        } else {
            (2, 6, 5, TNS_MAX_ORDER_LONG)
        };
        let _ = sampling_index;
        for w in 0..self.info.num_windows {
            let n_filt = r.read(filt_bits) as usize;
            self.tns.n_filt[w] = n_filt;
            if n_filt == 0 {
                continue;
            }
            let coef_res = r.read(1) + 3;
            for f in 0..n_filt {
                let filt = &mut self.tns.filt[w][f];
                filt.length = r.read(len_bits) as usize;
                let order = r.read(order_bits) as usize;
                if order > max_order {
                    return Err(malformed(format!("TNS order {order} above {max_order}")));
                }
                filt.order = order;
                if order == 0 {
                    continue;
                }
                filt.direction = r.bit();
                let compress = r.read(1);
                let bits = coef_res - compress;
                let mut parcor = [0f32; TNS_MAX_ORDER_LONG];
                for p in &mut parcor[..order] {
                    let raw = r.read(bits);
                    // Sign-extend from `bits`.
                    let c = ((raw << (32 - bits)) as i32) >> (32 - bits);
                    *p = tns_coef(coef_res, c);
                }
                filt.lpc = step_up(&parcor[..order]);
            }
        }
        Ok(())
    }

    fn read_spectrum(&mut self, r: &mut BitReader<'_>) -> Result<()> {
        let books: &Books = books();
        self.quant.fill(0);
        let info = &self.info;
        let mut group_start = 0;
        for g in 0..info.num_groups {
            let glen = info.group_len[g];
            for b in 0..info.max_sfb {
                let cb = self.band_cb[g][b];
                if cb == ZERO_HCB || cb >= NOISE_HCB {
                    continue;
                }
                let start = group_start + usize::from(info.swb[b]) * glen;
                let end = group_start + usize::from(info.swb[b + 1]) * glen;
                let book = &books.spectral[usize::from(cb) - 1];
                let dim = book.shape.dim;
                let mut k = start;
                while k < end {
                    let sym = book.book.decode(r) as usize;
                    let vals = book.values[sym];
                    for (j, &v) in vals[..dim].iter().enumerate() {
                        let mut v = i32::from(v);
                        if !book.shape.signed && v != 0 && r.bit() {
                            v = -v;
                        }
                        self.quant[k + j] = v;
                    }
                    if cb == ESC_HCB {
                        for q in &mut self.quant[k..k + 2] {
                            if q.abs() == 16 {
                                *q = q.signum() * escape(r)?;
                            }
                        }
                    }
                    k += dim;
                }
                if r.overrun() {
                    return Err(malformed("spectral data runs past the end of the block"));
                }
            }
            group_start += glen * 128;
        }
        Ok(())
    }

    /// Inverse quantisation and scaling, de-interleaving short windows into 128-line runs.
    /// Bands coded with the zero, noise or intensity books are left zero.
    fn dequantise(&mut self) -> Result<()> {
        let info = &self.info;
        let pow43 = pow43();
        let gains = gains();
        self.spec.fill(0.0);
        let mut window = 0;
        let mut group_start = 0;
        for g in 0..info.num_groups {
            let glen = info.group_len[g];
            for b in 0..info.max_sfb {
                let cb = self.band_cb[g][b];
                if cb == ZERO_HCB || cb >= NOISE_HCB {
                    continue;
                }
                let gain = gains[self.sf[g][b] as usize];
                let (lo, hi) = (usize::from(info.swb[b]), usize::from(info.swb[b + 1]));
                let width = hi - lo;
                for w in 0..glen {
                    let src = group_start + lo * glen + w * width;
                    let dst = (window + w) * 128 + lo;
                    for (o, &q) in self.spec[dst..dst + width]
                        .iter_mut()
                        .zip(&self.quant[src..src + width])
                    {
                        let m = pow43
                            .get(q.unsigned_abs() as usize)
                            .ok_or_else(|| malformed("quantised value above 8191"))?;
                        *o = if q < 0 { -m * gain } else { m * gain };
                    }
                }
            }
            window += glen;
            group_start += glen * 128;
        }
        Ok(())
    }

    /// The coefficient offset of band `b` in window `w` (short windows de-interleaved).
    pub(crate) fn band(&self, w: usize, b: usize) -> std::ops::Range<usize> {
        let base = if self.info.is_short() { w * 128 } else { 0 };
        base + usize::from(self.info.swb[b])..base + usize::from(self.info.swb[b + 1])
    }
}

/// escape_sequence: N leading ones, a zero, then N + 4 bits.
fn escape(r: &mut BitReader<'_>) -> Result<i32> {
    let mut n = 0;
    while r.bit() {
        n += 1;
        if n > 8 {
            return Err(malformed("escape sequence longer than 8"));
        }
    }
    Ok((1 << (n + 4)) + r.read(n + 4) as i32)
}

struct Pulses {
    start: usize,
    offsets: [(usize, i32); 4],
    count: usize,
}

fn read_pulses(r: &mut BitReader<'_>, info: &IcsInfo) -> Result<Pulses> {
    let count = r.read(2) as usize + 1;
    let start_sfb = r.read(6) as usize;
    if start_sfb >= info.swb.len() - 1 {
        return Err(malformed("pulse start band beyond the bands"));
    }
    let mut p = Pulses {
        start: usize::from(info.swb[start_sfb]),
        offsets: [(0, 0); 4],
        count,
    };
    for o in &mut p.offsets[..count] {
        *o = (r.read(5) as usize, r.read(4) as i32);
    }
    Ok(p)
}

impl Pulses {
    fn apply(&self, quant: &mut [i32]) -> Result<()> {
        let mut k = self.start;
        for &(offset, amp) in &self.offsets[..self.count] {
            k += offset;
            let q = quant
                .get_mut(k)
                .ok_or_else(|| malformed("pulse beyond the spectrum"))?;
            *q += if *q > 0 { amp } else { -amp };
        }
        Ok(())
    }
}

/// |q|^(4/3) for 0..=8191.
fn pow43() -> &'static [f32] {
    static T: OnceLock<Vec<f32>> = OnceLock::new();
    T.get_or_init(|| {
        (0..8192)
            .map(|i| {
                let x = f64::from(i);
                (x * libm::cbrt(x)) as f32
            })
            .collect()
    })
}

/// 2^(0.25 * (sf - 100)) for sf 0..=255.
fn gains() -> &'static [f32; 256] {
    static T: OnceLock<[f32; 256]> = OnceLock::new();
    T.get_or_init(|| std::array::from_fn(|sf| libm::exp2(0.25 * (sf as f64 - 100.0)) as f32))
}

/// A TNS coefficient from its index (14496-3 4.6.9.3).
fn tns_coef(coef_res: u32, c: i32) -> f32 {
    let half_pi = std::f64::consts::FRAC_PI_2;
    let base = f64::from(1u32 << (coef_res - 1));
    let iq = if c >= 0 {
        (base - 0.5) / half_pi
    } else {
        (base + 0.5) / half_pi
    };
    libm::sin(f64::from(c) / iq) as f32
}

/// Reflection coefficients to direct-form prediction coefficients a[1..=order].
fn step_up(parcor: &[f32]) -> [f32; TNS_MAX_ORDER_LONG] {
    let mut a = [0f32; TNS_MAX_ORDER_LONG + 1];
    a[0] = 1.0;
    for (m, &k) in parcor.iter().enumerate() {
        let m = m + 1;
        let b = a;
        for i in 1..m {
            a[i] = b[i] + k * b[m - i];
        }
        a[m] = k;
    }
    std::array::from_fn(|i| a[i + 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_decode_to_their_value() {
        // 0 ones: 16 + 4 bits; 1 one: 32 + 5 bits.
        let mut r = BitReader::new(&[0b0111_1100, 0b0001_0000]); // 0 1111 | 10 00001
        assert_eq!(escape(&mut r).unwrap(), 16 + 15);
        assert_eq!(escape(&mut r).unwrap(), 32 + 1);
        let mut r = BitReader::new(&[0xFF, 0xFF]);
        assert!(escape(&mut r).is_err());
    }

    #[test]
    fn grouping_bits_make_groups() {
        // EIGHT_SHORT, sine, max_sfb 0, grouping 1011000: groups of 2, 3, 1, 1, 1.
        let mut r = BitReader::new(&[0b0100_0000, 0b1011_0000]); // 0 10 0 0000 | 1011000
        let info = IcsInfo::read(&mut r, 4).unwrap();
        assert_eq!(info.num_groups, 5);
        assert_eq!(info.group_len[..5], [2, 3, 1, 1, 1]);
    }

    #[test]
    fn tns_coefficients_and_step_up() {
        assert_eq!(tns_coef(4, 0), 0.0);
        assert!(
            (tns_coef(4, 7) - libm::sin(7.0 / (7.5 / std::f64::consts::FRAC_PI_2)) as f32).abs()
                < 1e-7
        );
        let a = step_up(&[0.5]);
        assert_eq!(a[0], 0.5);
        let a = step_up(&[0.5, 0.25]);
        assert_eq!(&a[..2], &[0.5 + 0.25 * 0.5, 0.25]);
    }
}
