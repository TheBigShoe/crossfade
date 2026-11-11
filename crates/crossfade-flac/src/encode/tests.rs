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
    Analyzer::new(Level::new(level), false).analyze(x, 16, &mut plan);
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
    Analyzer::new(Level::new(5), false).analyze(&x, 24, &mut plan);
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

/// Three narrow resonances below 4 kHz (at a 192 kHz rate) and a few LSBs of noise, from
/// integer arithmetic only, as 24-bit stereo: strongly band-limited, so linear prediction can
/// reach far below the noise floor if its coefficients are fine enough.
fn band_limited(frames: usize) -> Vec<i32> {
    let mut state = [(0i64, 0i64); 3];
    let mut rng = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 33) as i64
    };
    let a = [32_751i64, 32_714, 32_557];
    let mut out = Vec::with_capacity(frames * 2);
    for _ in 0..frames {
        let mut sum = 0i64;
        for (s, &a) in state.iter_mut().zip(&a) {
            let kick = next() % 401 - 200;
            let y = ((s.0 * a) >> 14) - ((s.1 * 16_371) >> 14) + kick;
            *s = (y, s.0);
            sum += y;
        }
        for c in 0..2 {
            let v = sum * (7 + c) / 2 + next() % 7 - 3;
            out.push(v.clamp(-(1 << 23), (1 << 23) - 1) as i32);
        }
    }
    out
}

fn band_limited_size(precision: Option<u32>) -> usize {
    let samples = band_limited(4096 * 3);
    let mut enc = Encoder::new(EncoderConfig {
        sample_rate: 192_000,
        channels: 2,
        bits_per_sample: 24,
        level: 5,
    })
    .unwrap();
    enc.force_precision(precision);
    let mut out = Vec::new();
    enc.write(&samples, &mut out).unwrap();
    out.len()
}

/// What level 5 made of the signal above with 12-bit coefficients, the precision the encoder
/// used at every depth before it followed the reference encoder's choice.
#[test]
fn band_limited_high_rate_audio_needs_fine_coefficients() {
    let fine = band_limited_size(None);
    let coarse = band_limited_size(Some(12));
    assert!(
        fine * 5 <= coarse * 4,
        "{fine} bytes against {coarse} with 12-bit coefficients"
    );
    // Pinned from the encoder as fixed, plus 1%: a regression in precision fails this.
    const PINNED: usize = 1;
    assert!(fine <= PINNED + PINNED / 100, "{fine} bytes");
}
