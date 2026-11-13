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
                self.ints(input, 8, &mut v, |x| (x as i8 as u8) ^ 0x80);
                Samples::U8(v)
            }
            SampleFormat::S16 => {
                let mut v = reuse!(S16);
                self.ints(input, 16, &mut v, |x| x as i16);
                Samples::S16(v)
            }
            SampleFormat::S24 => {
                let mut v = reuse!(S24);
                self.ints(input, 24, &mut v, |x| x);
                Samples::S24(v)
            }
            SampleFormat::S32 => {
                let mut v = reuse!(S32);
                self.ints(input, 32, &mut v, |x| x);
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

    /// Appends `input` to `out` as `bits`-bit integers. The generator runs a block ahead of
    /// the arithmetic, so the arithmetic is one branch-free loop that vectorises; the draws
    /// are the same, one per sample in order.
    fn ints<T: Copy + Default>(
        &mut self,
        input: &[f64],
        bits: u32,
        out: &mut Vec<T>,
        to: impl Fn(i32) -> T,
    ) {
        const BLOCK: usize = 64;
        let scale = (1u64 << (bits - 1)) as f64;
        let (lo, hi) = (-scale, scale - 1.0);
        // Rounding is half away from zero, so `v` rounds outside lo..=hi exactly when it is at
        // most lo - 0.5 or at least hi + 0.5 (both exact in f64).
        let (below, above) = (lo - 0.5, hi + 0.5);
        let start = out.len();
        out.resize(start + input.len(), T::default());
        let out = &mut out[start..];
        let mut clipped = 0u64;
        let mut noise = [0f64; BLOCK];
        for (x, o) in input.chunks(BLOCK).zip(out.chunks_mut(BLOCK)) {
            let noise = &mut noise[..x.len()];
            match self.dither {
                Dither::Tpdf => noise.iter_mut().for_each(|d| *d = self.rng.tpdf()),
                Dither::None => noise.fill(0.0),
            }
            let dither = self.dither == Dither::Tpdf;
            for ((&x, o), &d) in x.iter().zip(o.iter_mut()).zip(noise.iter()) {
                // NaN is silence, as in `convert`.
                let v = if x.is_nan() { 0.0 } else { x * scale };
                clipped += u64::from(v <= below || v >= above);
                let y = if dither { (v + d).round() } else { v.round() };
                *o = to(y.clamp(lo, hi) as i32);
            }
        }
        self.clipped += clipped;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// The quantiser as it was before it was made to vectorise: one sample at a time.
    struct Reference {
        rng: SplitMix64,
        clipped: u64,
    }

    impl Reference {
        fn ints(&mut self, input: &[f64], bits: u32, dither: Dither, mut push: impl FnMut(i32)) {
            let scale = (1u64 << (bits - 1)) as f64;
            let (lo, hi) = (-scale, scale - 1.0);
            for &x in input {
                let v = if x.is_nan() { 0.0 } else { x * scale };
                let rounded = v.round();
                if !(lo..=hi).contains(&rounded) {
                    self.clipped += 1;
                }
                let r = self.rng.next();
                let d = ((r >> 32) as f64 - (r & 0xFFFF_FFFF) as f64) * (1.0 / 4_294_967_296.0);
                let y = match dither {
                    Dither::Tpdf => (v + d).round(),
                    Dither::None => rounded,
                };
                push(y.clamp(lo, hi) as i32);
            }
        }
    }

    /// Mostly ordinary samples, with the edges: exact half steps, full scale and past it,
    /// infinities and NaN.
    fn sample() -> impl Strategy<Value = f64> {
        prop_oneof![
            8 => -1.2f64..1.2,
            1 => (-70000i32..70000).prop_map(|k| f64::from(k) / 65536.0),
            1 => proptest::sample::select(vec![
                -1.0, 1.0, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, 0.0, -0.0,
                1.0 - 0.5 / 32768.0, -1.0 - 0.5 / 32768.0, 1.0 - 1.5 / 32768.0,
            ]),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(400))]
        /// The same bytes and clip count as the reference, chunk by chunk, every format.
        #[test]
        fn quantises_as_the_reference(
            chunks in proptest::collection::vec(proptest::collection::vec(sample(), 0..300), 1..5),
            format in 0usize..4,
            tpdf in proptest::bool::ANY,
        ) {
            let (format, bits) = [
                (SampleFormat::U8, 8),
                (SampleFormat::S16, 16),
                (SampleFormat::S24, 24),
                (SampleFormat::S32, 32),
            ][format];
            let dither = if tpdf { Dither::Tpdf } else { Dither::None };
            let mut q = Quantiser::new(format, dither);
            let mut r = Reference { rng: SplitMix64(SEED), clipped: 0 };
            for chunk in &chunks {
                let mut out = Samples::new(format);
                q.quantise(chunk, &mut out);
                let mut want = Vec::new();
                r.ints(chunk, bits, dither, |x| want.push(x));
                let got: Vec<i32> = match out {
                    Samples::U8(v) => v.iter().map(|&b| i32::from((b ^ 0x80) as i8)).collect(),
                    Samples::S16(v) => v.iter().map(|&s| i32::from(s)).collect(),
                    Samples::S24(v) | Samples::S32(v) => v,
                    other => panic!("{other:?}"),
                };
                prop_assert_eq!(got, want);
                prop_assert_eq!(q.clipped(), r.clipped);
            }
        }
    }
}
