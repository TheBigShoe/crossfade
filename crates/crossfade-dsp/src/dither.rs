//! Leaving the working format after a chain ran. Integer outputs get TPDF dither and
//! saturate, with a count of the samples the signal itself pushed past full scale; float
//! outputs are a plain cast, never dithered or clipped.
//!
//! The dither comes from SplitMix64 with a fixed seed, one draw per sample in stream order,
//! so the same input gives the same bytes on every platform, at every thread count and
//! however the stream is split into frames. Each draw's two 32-bit halves are two uniform
//! values whose difference is triangular on (-1, 1) LSB; the arithmetic is exact.

use crossfade_core::{SampleFormat, Samples};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dither {
    /// Triangular, two LSB peak to peak: the error is noise whose mean and power do not
    /// depend on the signal.
    #[default]
    Tpdf,
    /// Round half away from zero, as `convert` does.
    None,
}

impl Dither {
    pub const NAMES: [&'static str; 2] = ["tpdf", "none"];

    pub fn name(self) -> &'static str {
        match self {
            Self::Tpdf => "tpdf",
            Self::None => "none",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "tpdf" => Some(Self::Tpdf),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

const SEED: u64 = 0x63F5_DA7E_D17B_E25C;

struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// In LSB, on (-1, 1).
    fn tpdf(&mut self) -> f64 {
        let r = self.next();
        ((r >> 32) as f64 - (r & 0xFFFF_FFFF) as f64) * (1.0 / 4_294_967_296.0)
    }
}

/// `f64` samples to the output format, one stream's worth: the generator runs on across calls.
pub struct Quantiser {
    format: SampleFormat,
    dither: Dither,
    rng: SplitMix64,
    clipped: u64,
}

impl Quantiser {
    pub fn new(format: SampleFormat, dither: Dither) -> Self {
        Self {
            format,
            dither,
            rng: SplitMix64(SEED),
            clipped: 0,
        }
    }

    /// Samples the signal pushed outside the format's range so far. Dither alone moving a
    /// full-scale sample one step past the end is clamped without counting.
    pub fn clipped(&self) -> u64 {
        self.clipped
    }

    /// Replaces `out` with `input` in the output format, reusing its buffer when it already
    /// holds that format.
    pub fn quantise(&mut self, input: &[f64], out: &mut Samples) {
        // The old buffer, emptied, when it is the right kind; else a new one.
        macro_rules! reuse {
            ($variant:ident) => {
                match std::mem::replace(out, Samples::U8(Vec::new())) {
                    Samples::$variant(mut v) => {
                        v.clear();
                        v
                    }
                    _ => Vec::new(),
                }
            };
        }
        *out = match self.format {
            SampleFormat::U8 => {
                let mut v = reuse!(U8);
                self.ints(input, 8, |x| v.push((x as i8 as u8) ^ 0x80));
                Samples::U8(v)
            }
            SampleFormat::S16 => {
                let mut v = reuse!(S16);
                self.ints(input, 16, |x| v.push(x as i16));
                Samples::S16(v)
            }
            SampleFormat::S24 => {
                let mut v = reuse!(S24);
                self.ints(input, 24, |x| v.push(x));
                Samples::S24(v)
            }
            SampleFormat::S32 => {
                let mut v = reuse!(S32);
                self.ints(input, 32, |x| v.push(x));
                Samples::S32(v)
            }
            SampleFormat::F32 => {
                let mut v = reuse!(F32);
                v.extend(input.iter().map(|&x| x as f32));
                Samples::F32(v)
            }
            SampleFormat::F64 => {
                let mut v = reuse!(F64);
                v.extend_from_slice(input);
                Samples::F64(v)
            }
        };
    }

    fn ints(&mut self, input: &[f64], bits: u32, mut push: impl FnMut(i32)) {
        let scale = (1u64 << (bits - 1)) as f64;
        let (lo, hi) = (-scale, scale - 1.0);
        for &x in input {
            // NaN is silence, as in `convert`.
            let v = if x.is_nan() { 0.0 } else { x * scale };
            let rounded = v.round();
            if !(lo..=hi).contains(&rounded) {
                self.clipped += 1;
            }
            let y = match self.dither {
                Dither::Tpdf => (v + self.rng.tpdf()).round(),
                Dither::None => rounded,
            };
            push(y.clamp(lo, hi) as i32);
        }
    }
}
