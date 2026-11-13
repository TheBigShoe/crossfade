//! SILK's output resampler, initialised for decoding to 48 kHz (libopus `silk/resampler.c`,
//! `resampler_private_IIR_FIR.c`, `resampler_private_up2_HQ.c`): from 8, 12 or 16 kHz the
//! signal is up-sampled 2x by the all-pass IIR, then interpolated by the 8-tap polyphase FIR.
//! The input is delayed by `delay_matrix_dec` (0, 4 or 7 input samples) so that SILK lines up
//! with CELT. RFC 8251 section 5's buffer rule holds: the work buffer has room for twice the
//! batch plus the FIR order, and the FIR state moves as 16-bit samples.

use super::macros::{rshift_round, sat16, smlabb, smlawb, smulbb, smulwb, smulww};
use super::tables::{RESAMPLER_FRAC_FIR_12, RESAMPLER_UP2_HQ_0, RESAMPLER_UP2_HQ_1};

const ORDER_FIR_12: usize = 8;
const MAX_BATCH_SIZE_MS: usize = 10;
const MAX_FS_IN_KHZ: usize = 16;
const OUT_HZ: i32 = 48_000;

/// `silk_resampler_state_struct` for 8, 12 or 16 kHz to 48 kHz.
#[derive(Debug, Clone, Default)]
pub(super) struct Resampler {
    s_iir: [i32; 6],
    s_fir: [i16; ORDER_FIR_12],
    delay_buf: [i16; MAX_FS_IN_KHZ],
    batch_size: usize,
    inv_ratio_q16: i32,
    fs_in_khz: usize,
    fs_out_khz: usize,
    input_delay: usize,
}

impl Resampler {
    /// `silk_resampler_init(S, fs_in_hz, 48000, 0)`; `fs_in_hz` is 8000, 12000 or 16000.
    pub(super) fn new(fs_in_hz: i32) -> Self {
        let input_delay = match fs_in_hz {
            8000 => 0,
            12000 => 4,
            _ => 7,
        };
        let fs_in_khz = (fs_in_hz / 1000) as usize;
        // 2x up-sampling first, then the fractional FIR (`up2x = 1`).
        let mut inv_ratio_q16 = ((fs_in_hz << 15) / OUT_HZ) << 2;
        while smulww(inv_ratio_q16, OUT_HZ) < fs_in_hz << 1 {
            inv_ratio_q16 += 1;
        }
        Self {
            batch_size: fs_in_khz * MAX_BATCH_SIZE_MS,
            inv_ratio_q16,
            fs_in_khz,
            fs_out_khz: (OUT_HZ / 1000) as usize,
            input_delay,
            ..Self::default()
        }
    }

    /// The input delay in input samples (`inputDelay`).
    #[cfg(test)]
    pub(super) fn input_delay(&self) -> usize {
        self.input_delay
    }

    /// `silk_resampler`: `input` (at least 1 ms) to `out` (`input.len() * 48 / fs_in_khz`
    /// samples).
    pub(super) fn process(&mut self, out: &mut [i16], input: &[i16]) {
        let fs_in = self.fs_in_khz;
        if fs_in == 0 || input.len() < fs_in {
            // Never initialised, or less than the delay line holds: nothing a decoder sends.
            out.fill(0);
            return;
        }
        let n_samples = fs_in - self.input_delay;
        self.delay_buf[self.input_delay..fs_in].copy_from_slice(&input[..n_samples]);
        let delay = self.delay_buf;
        let first = self.fs_out_khz.min(out.len());
        let (head, tail) = out.split_at_mut(first);
        self.iir_fir(head, &delay[..fs_in]);
        self.iir_fir(tail, &input[n_samples..input.len() - self.input_delay]);
        let d = self.input_delay;
        self.delay_buf[..d].copy_from_slice(&input[input.len() - d..]);
    }

    /// `silk_resampler_private_IIR_FIR`.
    fn iir_fir(&mut self, out: &mut [i16], mut input: &[i16]) {
        let mut buf = [0i16; 2 * MAX_BATCH_SIZE_MS * MAX_FS_IN_KHZ + ORDER_FIR_12];
        buf[..ORDER_FIR_12].copy_from_slice(&self.s_fir);
        let mut written = 0;
        let mut n;
        loop {
            n = input.len().min(self.batch_size);
            up2_hq(
                &mut self.s_iir,
                &mut buf[ORDER_FIR_12..ORDER_FIR_12 + 2 * n],
                &input[..n],
            );
            let max_index_q16 = (n as i32) << 17;
            let at = written.min(out.len());
            written += interpolate(&mut out[at..], &buf, max_index_q16, self.inv_ratio_q16);
            input = &input[n..];
            if input.is_empty() {
                break;
            }
            buf.copy_within(2 * n..2 * n + ORDER_FIR_12, 0);
        }
        self.s_fir
            .copy_from_slice(&buf[2 * n..2 * n + ORDER_FIR_12]);
    }
}

/// `silk_resampler_private_up2_HQ`: 2x up-sampling by two three-section all-pass chains.
fn up2_hq(s: &mut [i32; 6], out: &mut [i16], input: &[i16]) {
    let section = |s: &mut [i32], c: &[i16; 3], in32: i32| {
        let y = in32.wrapping_sub(s[0]);
        let x = smulwb(y, c[0].into());
        let out1 = s[0].wrapping_add(x);
        s[0] = in32.wrapping_add(x);
        let y = out1.wrapping_sub(s[1]);
        let x = smulwb(y, c[1].into());
        let out2 = s[1].wrapping_add(x);
        s[1] = out1.wrapping_add(x);
        let y = out2.wrapping_sub(s[2]);
        let x = smlawb(y, y, c[2].into());
        let out1 = s[2].wrapping_add(x);
        s[2] = out2.wrapping_add(x);
        sat16(rshift_round(out1, 10))
    };
    for (pair, &x) in out.as_chunks_mut::<2>().0.iter_mut().zip(input) {
        let in32 = i32::from(x) << 10;
        let (s0, s1) = s.split_at_mut(3);
        pair[0] = section(s0, &RESAMPLER_UP2_HQ_0, in32);
        pair[1] = section(s1, &RESAMPLER_UP2_HQ_1, in32);
    }
}

/// `silk_resampler_private_IIR_FIR_INTERPOL`: returns the samples written.
fn interpolate(out: &mut [i16], buf: &[i16], max_index_q16: i32, step_q16: i32) -> usize {
    let mut n = 0;
    let mut index_q16 = 0;
    while index_q16 < max_index_q16 && n < out.len() {
        let t = smulwb(index_q16 & 0xFFFF, 12) as usize;
        let b = &buf[(index_q16 >> 16) as usize..][..ORDER_FIR_12];
        let (f, g) = (&RESAMPLER_FRAC_FIR_12[t], &RESAMPLER_FRAC_FIR_12[11 - t]);
        let mut res = smulbb(b[0].into(), f[0].into());
        res = smlabb(res, b[1].into(), f[1].into());
        res = smlabb(res, b[2].into(), f[2].into());
        res = smlabb(res, b[3].into(), f[3].into());
        res = smlabb(res, b[4].into(), g[3].into());
        res = smlabb(res, b[5].into(), g[2].into());
        res = smlabb(res, b[6].into(), g[1].into());
        res = smlabb(res, b[7].into(), g[0].into());
        out[n] = sat16(rshift_round(res, 15));
        n += 1;
        index_q16 += step_q16;
    }
    n
}
