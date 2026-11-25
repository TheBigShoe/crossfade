//! The encoder's resampler from 48 kHz down to SILK's internal rate (libopus
//! `silk/resampler.c` initialised for encoding, `resampler_private_down_FIR.c`,
//! `resampler_private_AR2.c`): a second-order AR filter and a symmetric FIR of order 36,
//! for the ratios 1/3, 1/4 and 1/6.

use super::fix::{rshift_round, sat16, smlawb, smulwb};
use super::pitch::resampler_ar2;

const ORDER_FIR2: usize = 36;
const MAX_BATCH_SIZE_MS: usize = 10;
const IN_HZ: i32 = 48_000;

static COEFS_1_3: [i16; 2 + ORDER_FIR2 / 2] = [
    16102, -15162, -13, 0, 20, 26, 5, -31, -43, -4, 65, 90, 7, -157, -248, -44, 593, 1583, 2612,
    3271,
];
static COEFS_1_4: [i16; 2 + ORDER_FIR2 / 2] = [
    22500, -15099, 3, -14, -20, -15, 2, 25, 37, 25, -16, -71, -107, -79, 50, 292, 623, 982, 1288,
    1464,
];
static COEFS_1_6: [i16; 2 + ORDER_FIR2 / 2] = [
    27540, -15257, 17, 12, 8, 1, -10, -22, -30, -32, -22, 3, 44, 100, 168, 243, 317, 381, 429, 455,
];

/// `silk_resampler_state_struct` for 48 kHz to 8, 12 or 16 kHz.
#[derive(Debug, Clone)]
pub(super) struct DownResampler {
    s_iir: [i32; 2],
    s_fir: [i32; ORDER_FIR2],
    delay_buf: [i16; 48],
    inv_ratio_q16: i32,
    fs_out_khz: usize,
    input_delay: usize,
    coefs: &'static [i16; 2 + ORDER_FIR2 / 2],
}

impl Default for DownResampler {
    fn default() -> Self {
        Self::new(16_000)
    }
}

impl DownResampler {
    /// `silk_resampler_init(S, 48000, fs_out_hz, 1)`.
    pub(super) fn new(fs_out_hz: i32) -> Self {
        let (coefs, input_delay) = match fs_out_hz {
            8000 => (&COEFS_1_6, 18),
            12000 => (&COEFS_1_4, 10),
            _ => (&COEFS_1_3, 12),
        };
        let mut inv_ratio_q16 = ((IN_HZ << 14) / fs_out_hz) << 2;
        while super::fix::smulww(inv_ratio_q16, fs_out_hz) < IN_HZ {
            inv_ratio_q16 += 1;
        }
        Self {
            s_iir: [0; 2],
            s_fir: [0; ORDER_FIR2],
            delay_buf: [0; 48],
            inv_ratio_q16,
            fs_out_khz: (fs_out_hz / 1000) as usize,
            input_delay,
            coefs,
        }
    }

    /// `silk_resampler`: `input` (at least 1 ms) at 48 kHz to `out` at the internal rate.
    pub(super) fn process(&mut self, out: &mut [i16], input: &[i16]) {
        const FS_IN_KHZ: usize = 48;
        let n_samples = FS_IN_KHZ - self.input_delay;
        self.delay_buf[self.input_delay..].copy_from_slice(&input[..n_samples]);
        let delay = self.delay_buf;
        self.down_fir(out, &delay);
        let rest = input.len() - FS_IN_KHZ;
        self.down_fir(
            &mut out[self.fs_out_khz..],
            &input[n_samples..n_samples + rest],
        );
        let d = self.input_delay;
        self.delay_buf[..d].copy_from_slice(&input[input.len() - d..]);
    }

    /// `silk_resampler_private_down_FIR`.
    fn down_fir(&mut self, out: &mut [i16], mut input: &[i16]) {
        const BATCH: usize = 48 * MAX_BATCH_SIZE_MS;
        let mut buf = [0i32; BATCH + ORDER_FIR2];
        buf[..ORDER_FIR2].copy_from_slice(&self.s_fir);
        let fir = &self.coefs[2..];
        let a: [i32; 2] = [self.coefs[0].into(), self.coefs[1].into()];
        let mut o = 0;
        let mut n_in;
        loop {
            n_in = input.len().min(BATCH);
            resampler_ar2(
                &mut self.s_iir,
                &mut buf[ORDER_FIR2..ORDER_FIR2 + n_in],
                &input[..n_in],
                &a,
            );
            let max_index_q16 = (n_in as i32) << 16;
            let mut index_q16 = 0;
            while index_q16 < max_index_q16 {
                let p = &buf[(index_q16 >> 16) as usize..];
                let mut res_q6 = smulwb(p[0].wrapping_add(p[35]), fir[0].into());
                for k in 1..ORDER_FIR2 / 2 {
                    res_q6 = smlawb(res_q6, p[k].wrapping_add(p[35 - k]), fir[k].into());
                }
                out[o] = sat16(rshift_round(res_q6, 6));
                o += 1;
                index_q16 += self.inv_ratio_q16;
            }
            input = &input[n_in..];
            if input.len() > 1 {
                buf.copy_within(n_in..n_in + ORDER_FIR2, 0);
            } else {
                break;
            }
        }
        self.s_fir.copy_from_slice(&buf[n_in..n_in + ORDER_FIR2]);
    }
}
