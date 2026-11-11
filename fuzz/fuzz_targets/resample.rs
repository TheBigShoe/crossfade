#![no_main]
//! The resampler's contract under arbitrary rates, channel counts, presets, chunkings and
//! sample values (NaN, infinities and subnormals included): no panic; exactly
//! `output_len(frames)` frames; the same bits however the input is split; memory fixed after
//! construction; and finite input within +-1e300 gives finite output (non-finite input only
//! spoils the outputs within the kernel's reach, which the unit tests check). Panics by
//! design when any of that fails.

use crossfade_resample::{MAX_RATE, Quality, Resampler, output_len};
use libfuzzer_sys::fuzz_target;

/// Common and awkward rates; a raw 20-bit value is used when the selector is past the end.
const RATES: [u32; 12] = [
    1, 7, 8_000, 22_050, 44_100, 44_101, 48_000, 96_000, 192_000, 999_983, 1_000_003, MAX_RATE,
];
/// Output samples per run at most, so one input frame at 1 Hz -> 1 MHz stays cheap.
const MAX_OUT: u64 = 1 << 17;

fn rate(sel: u8, raw: [u8; 3]) -> u32 {
    RATES.get(usize::from(sel)).copied().unwrap_or_else(|| {
        let v = u32::from_le_bytes([raw[0], raw[1], raw[2] & 0x0f, 0]);
        v.clamp(1, MAX_RATE)
    })
}

fuzz_target!(|data: &[u8]| {
    let [a_sel, a0, a1, a2, b_sel, b0, b1, b2, channels, quality, cut, body @ ..] = data else {
        return;
    };
    let (a, b) = (rate(a_sel % 16, [*a0, *a1, *a2]), rate(b_sel % 16, [*b0, *b1, *b2]));
    let channels = 1 + usize::from(channels % 8);
    let quality = [Quality::Fast, Quality::Balanced, Quality::Best][usize::from(quality % 3)];

    // Samples: raw f64 bits, so NaN, infinities and subnormals all occur.
    let mut x: Vec<f64> = body
        .chunks_exact(8)
        .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
        .collect();
    let mut frames = x.len() / channels;
    while frames > 0 && output_len(frames as u64, a, b) * channels as u64 > MAX_OUT {
        frames /= 2;
    }
    x.truncate(frames * channels);

    let mut r = Resampler::new(a, b, channels, quality).unwrap();
    let memory = r.memory_bytes();
    let mut whole = Vec::new();
    r.process(&x, &mut whole).unwrap();
    r.flush(&mut whole);
    assert_eq!(whole.len() as u64, output_len(frames as u64, a, b) * channels as u64);

    // Split at varying sizes (zero included); flush has reset the resampler.
    let mut split = Vec::new();
    let mut at = 0;
    let mut step = usize::from(*cut);
    while at < frames {
        let n = (step % 97).min(frames - at);
        r.process(&x[at * channels..(at + n) * channels], &mut split).unwrap();
        at += n;
        step = step.wrapping_mul(31).wrapping_add(7);
    }
    r.flush(&mut split);
    assert_eq!(r.memory_bytes(), memory);
    let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert!(bits(&split) == bits(&whole), "chunked output differs");

    if x.iter().all(|v| v.is_finite() && v.abs() <= 1e300) {
        assert!(whole.iter().all(|v| v.is_finite()), "finite input gave non-finite output");
    }
});
