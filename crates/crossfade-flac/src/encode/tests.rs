//! Unit tests that reach inside the encoder: forced stereo modes.

use super::*;
use crate::Decoder;
use crate::frame::ChannelAssignment::{LeftSide, MidSide, SideRight};

fn pseudo(bits: u8, count: usize, seed: u64) -> Vec<i32> {
    let mut s = seed | 1;
    (0..count)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s as i64 >> (64 - u32::from(bits))) as i32
        })
        .collect()
}

/// Encodes one block with the stereo mode forced and decodes it again.
fn forced(bits: u8, level: u8, mode: ChannelAssignment, samples: &[i32]) {
    let mut enc = Encoder::new(EncoderConfig {
        sample_rate: 44_100,
        channels: 2,
        bits_per_sample: bits,
        level,
    })
    .unwrap();
    enc.force_stereo(mode);
    let mut out = Vec::new();
    enc.encode_block(samples, &mut out).unwrap();
    let info = enc.finish();
    let header = frame::parse_header(&out).unwrap();
    assert_eq!(header.channels, mode, "bits {bits} level {level}");
    let mut dec = Decoder::new(&info).unwrap();
    let mut back = Vec::new();
    dec.decode_frame(&out).unwrap().interleave_i32(0, &mut back);
    assert_eq!(back, samples, "bits {bits} level {level} {mode:?}");
    assert_eq!(dec.finish().unwrap(), crate::Verification::Md5Matched);
}

#[test]
fn every_stereo_mode_round_trips_at_every_depth() {
    for mode in [LeftSide, SideRight, MidSide] {
        for bits in [4u8, 8, 16, 24, 31, 32] {
            let lo = if bits == 32 {
                i32::MIN
            } else {
                -(1 << (bits - 1))
            };
            let hi = if bits == 32 {
                i32::MAX
            } else {
                (1 << (bits - 1)) - 1
            };
            let mut signals = vec![pseudo(bits, 2 * 300, 11)];
            // Extremes make the side channel as wide as it gets (33 bits at 32-bit depth).
            signals.push((0..200).map(|i| if i % 4 < 2 { lo } else { hi }).collect());
            signals.push([lo, hi].repeat(50));
            signals.push([hi, lo].repeat(50));
            signals.push([lo, lo].repeat(40));
            signals.push([hi, lo, lo, hi, hi, hi, lo, lo].to_vec());
            signals.push([1, 0, 0, 1, -1, 0, 0, -1].repeat(9));
            for s in &signals {
                for level in [1, 5, 8] {
                    forced(bits, level, mode, s);
                }
            }
        }
    }
}

use super::analyze::{Analyzer, Kind, Level, Plan};
use super::rice::RAW;

fn analyze16(level: u8, x: &[i32]) -> Plan {
    let mut plan = Plan::new(4096);
    Analyzer::new(Level::new(level), 16, false).analyze(x, 16, &mut plan);
    plan
}

#[test]
fn each_subframe_kind_is_chosen_where_it_belongs() {
    assert_eq!(analyze16(5, &[7; 100]).kind, Kind::Constant);
    let noise = pseudo(16, 4096, 3);
    assert_eq!(analyze16(5, &noise).kind, Kind::Verbatim);
    let ramp: Vec<i32> = (0..4096).map(|i| i * 3).collect();
    assert_eq!(analyze16(0, &ramp).kind, Kind::Fixed(2));
    let tone: Vec<i32> = (0..4096)
        .map(|i| {
            (f64::from(i) * 0.07)
                .sin()
                .mul_add(9000.0, f64::from(i % 5)) as i32
        })
        .collect();
    assert!(matches!(analyze16(5, &tone).kind, Kind::Lpc(_)));
    // A wasted-bit signal keeps its shift and a smaller depth.
    let shifted: Vec<i32> = pseudo(10, 512, 4).iter().map(|&v| v << 4).collect();
    let plan = analyze16(5, &shifted);
    assert!(plan.wasted >= 4 && plan.bps + plan.wasted == 16);
}

#[test]
fn raw_partitions_are_used_when_cheaper() {
    // Silence, then full-scale noise: the quiet half costs nothing, the loud half is better
    // stored than Rice coded.
    let mut x = vec![0; 2048];
    x.extend(pseudo(16, 2048, 8));
    let plan = analyze16(5, &x);
    assert_eq!(plan.kind, Kind::Fixed(0));
    assert!(plan.rice.params[..1 << plan.rice.order].contains(&RAW));
    assert!(plan.bits < 4096 * 16 / 2 + 200, "{}", plan.bits);
    // And the decoder reads it back.
    let samples: Vec<i32> = x.iter().flat_map(|&v| [v, v]).collect();
    let mut enc = Encoder::new(EncoderConfig::new(44_100, 2, 16)).unwrap();
    let mut out = Vec::new();
    enc.encode_block(&samples, &mut out).unwrap();
    let info = enc.finish();
    let mut dec = Decoder::new(&info).unwrap();
    let mut back = Vec::new();
    dec.decode_frame(&out).unwrap().interleave_i32(0, &mut back);
    assert_eq!(back, samples);
}

#[test]
fn large_residuals_get_five_bit_parameters() {
    // 24-bit audio with a 20-bit noise floor: residuals need Rice parameters above 14.
    let x: Vec<i32> = pseudo(21, 4096, 9)
        .iter()
        .enumerate()
        .map(|(i, &v)| v + (i as i32 % 64) * 100_000)
        .collect();
    let mut plan = Plan::new(4096);
    Analyzer::new(Level::new(5), 24, false).analyze(&x, 24, &mut plan);
    assert!(plan.rice.method5, "{:?}", plan.kind);
    assert!(
        plan.rice.params[..1 << plan.rice.order]
            .iter()
            .any(|&k| k > 14 && k != RAW)
    );
}

#[test]
fn steady_state_does_not_grow_any_buffer() {
    for (channels, bits, level) in [(2u8, 32u8, 8u8), (2, 24, 5), (3, 16, 6), (1, 16, 3)] {
        let mut enc = Encoder::new(EncoderConfig {
            sample_rate: 44_100,
            channels,
            bits_per_sample: bits,
            level,
        })
        .unwrap();
        let mut out = Vec::new();
        let chunk = pseudo(bits, enc.block_size() * usize::from(channels) * 2 + 6, 1);
        let snapshot = |e: &Encoder| {
            let mut v = e.analyzer.capacities();
            v.push(e.pending.capacity());
            v.push(e.md5_bytes.capacity());
            v.extend(e.planes.iter().map(Vec::capacity));
            v.extend([e.mid.capacity(), e.side32.capacity(), e.side64.capacity()]);
            v.extend(e.slots.iter().map(|s| s.res.capacity()));
            v
        };
        enc.write(&chunk, &mut out).unwrap();
        let before = snapshot(&enc);
        for _ in 0..6 {
            enc.write(&chunk, &mut out).unwrap();
        }
        assert_eq!(
            snapshot(&enc),
            before,
            "{channels} ch {bits} bit level {level}"
        );
    }
}

/// Three sines below 4 kHz (at a 192 kHz rate) and a few LSBs of noise, as 24-bit stereo:
/// strongly band-limited, so linear prediction can reach far below the noise floor if its
/// coefficients are fine enough. `sin` is fine here (a test input, not the encoder); the
/// pinned size has slack for any platform difference in the last bit.
fn band_limited(frames: usize) -> Vec<i32> {
    let mut rng = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 33) as i64
    };
    let mut out = Vec::with_capacity(frames * 2);
    for t in 0..frames {
        let x = t as f64 / 192_000.0;
        let tau = std::f64::consts::TAU;
        for c in 0..2 {
            let phase = f64::from(c) * 0.3;
            let v = 0.3 * (tau * 440.0 * x + phase).sin()
                + 0.2 * (tau * 1300.0 * x).sin()
                + 0.1 * (tau * 3100.0 * x + phase).sin();
            let noisy = (v * 8_388_607.0) as i64 + next() % 7 - 3;
            out.push(noisy.clamp(-(1 << 23), (1 << 23) - 1) as i32);
        }
    }
    out
}

fn band_limited_size() -> usize {
    let samples = band_limited(4096 * 3);
    let mut enc = Encoder::new(EncoderConfig {
        sample_rate: 192_000,
        channels: 2,
        bits_per_sample: 24,
        level: 5,
    })
    .unwrap();
    let mut out = Vec::new();
    enc.write(&samples, &mut out).unwrap();
    out.len()
}

/// Level 5 on the signal above, as the encoder was before it followed the reference
/// encoder's choice of coefficient precision (12 bits at every depth).
const SIZE_WITH_12_BIT_COEFFICIENTS: usize = 21_931;
/// The encoder as fixed (15-bit coefficients at 24-bit depth, a ridge search and a refined
/// quantiser on predictable blocks).
const PINNED: usize = 17_260;

#[test]
fn band_limited_high_rate_audio_needs_fine_coefficients() {
    let size = band_limited_size();
    assert!(
        size * 5 <= SIZE_WITH_12_BIT_COEFFICIENTS * 4,
        "{size} bytes, 12-bit coefficients gave {SIZE_WITH_12_BIT_COEFFICIENTS}"
    );
    // Pinned from the fixed encoder plus 1%: a regression in precision fails this.
    assert!(size <= PINNED + PINNED / 100, "{size} bytes");
}

/// The search with lower bounds and fixed predictors last picks exactly what coding every
/// candidate in the old order picked, ties included: same kind, same bits, same residual.
#[test]
fn bounded_search_chooses_what_the_exhaustive_order_chose() {
    let mut cases: Vec<(u32, Vec<i32>)> = Vec::new();
    for (i, bits) in [9u32, 16, 16, 24, 24].into_iter().enumerate() {
        cases.push((bits, pseudo(bits as u8, 4096, i as u64 + 20)));
        // Smooth signals, where fixed predictors and LPC are close and ties happen.
        let ramp: Vec<i32> = (0..4096)
            .map(|t| ((t * 37 + i as i32 * 11) % 401 - 200) * (1 << (bits - 9)))
            .collect();
        cases.push((bits, ramp));
        let quiet: Vec<i32> = pseudo(4, 1152, i as u64 + 40);
        cases.push((bits, quiet));
    }
    cases.push((16, band_limited(4096).iter().map(|&v| v >> 8).collect()));
    // Short blocks of small values, where candidates often tie to the bit.
    let mut s = 0x5eed_u64;
    for i in 0..4000usize {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let n = 5 + (s % 100) as usize;
        let range = [2i32, 3, 5, 9, 17, 40][i % 6];
        let step = (i % 3) as i32;
        let x: Vec<i32> = (0..n)
            .map(|t| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                t as i32 * step + (s % range as u64) as i32 - range / 2
            })
            .collect();
        cases.push((8, x));
    }
    cases.push((24, band_limited(4096).iter().step_by(2).copied().collect()));
    let (mut fast, mut slow) = (Plan::new(4096), Plan::new(4096));
    for level in 0..=8 {
        let mut analyzers = std::collections::HashMap::new();
        for (bits, x) in &cases {
            let (search, reference) = analyzers.entry(*bits).or_insert_with(|| {
                let mut reference = Analyzer::new(Level::new(level), *bits, false);
                reference.reference = true;
                (Analyzer::new(Level::new(level), *bits, false), reference)
            });
            search.analyze(x, *bits, &mut fast);
            reference.analyze(x, *bits, &mut slow);
            assert_eq!(
                (fast.kind, fast.bits, &fast.res),
                (slow.kind, slow.bits, &slow.res),
                "level {level} bits {bits}"
            );
        }
    }
}

/// How many LPC candidates each level codes for real, an explicit table. Ordinary audio: the
/// best `lpc_exact` of each window's orders by predicted bits, nothing from the pool. A block
/// as predictable as a pure tone also refines and, from level 5, searches ridges: the pool's
/// best two by predicted bits are coded on top.
#[test]
fn exact_lpc_codings_per_level() {
    let music: Vec<i32> = (0..4096)
        .map(|t| {
            let t = f64::from(t);
            let tone =
                (t * 0.06).sin() * 0.4 + (t * 0.0007).sin() * 0.25 + (t * 0.0213).sin() * 0.1;
            (tone * 20_000.0) as i32
        })
        .zip(pseudo(10, 4096, 5))
        .map(|(a, b)| a + b)
        .collect();
    // A 1 kHz sine at 192 kHz, 24 bits: about as predictable as audio gets.
    let tone: Vec<i32> = (0..4096)
        .map(|t| {
            let phase = f64::from(t) * std::f64::consts::TAU * 1000.0 / 192_000.0;
            (phase.sin() * 0.99 * 8_388_607.0) as i32
        })
        .collect();
    let window = [0usize, 0, 0, 1, 1, 2, 3, 2, 3];
    let pool = [0usize, 0, 0, 1, 1, 2, 2, 2, 2];
    for level in 0..=8u8 {
        let mut plan = Plan::new(4096);
        let mut a = Analyzer::new(Level::new(level), 16, false);
        a.analyze(&music, 16, &mut plan);
        let l = usize::from(level);
        assert_eq!(
            (a.window_codings, a.pool_codings),
            (window[l], 0),
            "music, level {level}"
        );
        let mut a = Analyzer::new(Level::new(level), 24, false);
        a.analyze(&tone, 24, &mut plan);
        assert_eq!(
            (a.window_codings, a.pool_codings),
            (window[l], pool[l]),
            "tone, level {level}"
        );
    }
}
