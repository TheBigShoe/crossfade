//! Sample-format conversion. These rules are the project's definition:
//!
//! - Same format: copy.
//! - Integer to a wider integer: left shift by the difference in bits (exact).
//! - Integer to a narrower integer: arithmetic right shift (truncation; dither
//!   is a separate, later filter).
//! - `U8` is offset binary: it converts as `(v ^ 0x80) as i8`.
//! - Integer to float: `value / 2^(bits-1)`.
//! - Float to integer: multiply by `2^(bits-1)`, round half away from zero,
//!   clamp to the format's range; NaN becomes 0. Bit for bit what the test
//!   kit's PCM oracle does.
//! - Float to float: plain cast.
//!
//! The `(from, to)` pair is matched once per call; each arm is a tight loop
//! over the slices with the shifts and scales fixed at compile time.

use crossfade_core::{AudioFrame, Error, SampleFormat, Samples};

/// Converts `input` into `out`, which receives the input's rate, layout and
/// pts. `out`'s buffer is reused (cleared, capacity kept) when it already
/// holds the target format.
pub fn convert_format(
    input: &AudioFrame,
    to: SampleFormat,
    out: &mut AudioFrame,
) -> Result<(), Error> {
    out.sample_rate = input.sample_rate;
    out.layout.clone_from(&input.layout);
    out.pts = input.pts;

    // Take the old buffer out first: the arms need it by value and the
    // result replaces it, so a failure can never leave stale samples.
    let old = std::mem::replace(&mut out.samples, Samples::U8(Vec::new()));
    out.samples = match &input.samples {
        Samples::U8(s) => from_int(s, to, old, 8, |x| i32::from((x ^ 0x80) as i8)),
        Samples::S16(s) => from_int(s, to, old, 16, i32::from),
        Samples::S24(s) => from_int(s, to, old, 24, |x| x),
        Samples::S32(s) => from_int(s, to, old, 32, |x| x),
        Samples::F32(s) => from_float(s, to, old, f64::from),
        Samples::F64(s) => from_float(s, to, old, |x| x),
    };
    Ok(())
}

fn fill<A: Copy, B>(src: &[A], mut buf: Vec<B>, f: impl Fn(A) -> B) -> Vec<B> {
    buf.clear();
    buf.extend(src.iter().map(|&x| f(x)));
    buf
}

/// Dispatches on the target format once, then runs one monomorphic loop.
macro_rules! dispatch {
    ($src:expr, $to:expr, $old:expr, $int:expr, $f32:expr, $f64:expr) => {
        match $to {
            SampleFormat::U8 => {
                let buf = match $old {
                    Samples::U8(v) => v,
                    _ => Vec::new(),
                };
                Samples::U8(fill($src, buf, |x| ($int(x, 8) as i8 as u8) ^ 0x80))
            }
            SampleFormat::S16 => {
                let buf = match $old {
                    Samples::S16(v) => v,
                    _ => Vec::new(),
                };
                Samples::S16(fill($src, buf, |x| $int(x, 16) as i16))
            }
            SampleFormat::S24 => {
                let buf = match $old {
                    Samples::S24(v) => v,
                    _ => Vec::new(),
                };
                Samples::S24(fill($src, buf, |x| $int(x, 24)))
            }
            SampleFormat::S32 => {
                let buf = match $old {
                    Samples::S32(v) => v,
                    _ => Vec::new(),
                };
                Samples::S32(fill($src, buf, |x| $int(x, 32)))
            }
            SampleFormat::F32 => {
                let buf = match $old {
                    Samples::F32(v) => v,
                    _ => Vec::new(),
                };
                Samples::F32(fill($src, buf, $f32))
            }
            SampleFormat::F64 => {
                let buf = match $old {
                    Samples::F64(v) => v,
                    _ => Vec::new(),
                };
                Samples::F64(fill($src, buf, $f64))
            }
        }
    };
}

/// `bits` is a literal at every call site; inlining folds it, so each loop
/// body is a shift or a multiply by a constant.
#[inline(always)]
fn rescale(s: i32, from: u32, to: u32) -> i32 {
    if to >= from {
        s << (to - from)
    } else {
        s >> (from - to)
    }
}

#[inline(always)]
fn from_int<A: Copy>(
    src: &[A],
    to: SampleFormat,
    old: Samples,
    from_bits: u32,
    load: impl Fn(A) -> i32 + Copy,
) -> Samples {
    // Scales are exact powers of two, so multiplying equals dividing.
    let scale32 = 1.0 / (1u64 << (from_bits - 1)) as f32;
    let scale64 = 1.0 / (1u64 << (from_bits - 1)) as f64;
    dispatch!(
        src,
        to,
        old,
        |x: A, to_bits: u32| rescale(load(x), from_bits, to_bits),
        |x: A| load(x) as f32 * scale32,
        |x: A| f64::from(load(x)) * scale64
    )
}

/// Round half away from zero, clamp to the format's range; NaN gives 0.
#[inline(always)]
fn quantise(x: f64, bits: u32) -> i32 {
    let scale = (1u64 << (bits - 1)) as f64;
    (x * scale).round().clamp(-scale, scale - 1.0) as i32
}

#[inline(always)]
fn from_float<A: Copy>(
    src: &[A],
    to: SampleFormat,
    old: Samples,
    load: impl Fn(A) -> f64 + Copy,
) -> Samples {
    dispatch!(
        src,
        to,
        old,
        |x: A, bits: u32| quantise(load(x), bits),
        |x: A| load(x) as f32,
        |x: A| load(x)
    )
}
