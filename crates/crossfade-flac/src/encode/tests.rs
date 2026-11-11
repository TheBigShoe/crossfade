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
        threads: 1,
    })
    .unwrap();
    enc.coder.force_stereo(mode);
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
            threads: 1,
        })
        .unwrap();
        let mut out = Vec::new();
        let chunk = pseudo(bits, enc.block_size() * usize::from(channels) * 2 + 6, 1);
        let snapshot = |e: &Encoder| {
            let c = &e.coder;
            let mut v = c.analyzer.capacities();
            v.push(e.pending.capacity());
            v.push(e.hasher.bytes.capacity());
            v.extend(c.planes.iter().map(Vec::capacity));
            v.extend([c.mid.capacity(), c.side32.capacity(), c.side64.capacity()]);
            v.extend(c.slots.iter().map(|s| s.res.capacity()));
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
        threads: 1,
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
/// as predictable as a pure tone codes all of a window's orders, refines and, from level 5,
/// searches ridges: the pool's best by predicted bits is coded on top.
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
    // Ordinary audio: the best few of each window's orders.
    let window = [0usize, 0, 0, 1, 1, 2, 3, 2, 3];
    // A window whose coded fits all land far above their estimates codes the rest of its
    // orders too (levels 7 and 8 try more than they code); the pool's best one is coded.
    let tone_window = [0usize, 0, 0, 1, 1, 2, 3, 4, 5];
    let pool = [0usize, 0, 0, 1, 1, 1, 1, 1, 1];
    let ridges = [0usize, 0, 0, 0, 0, 2, 2, 3, 4];
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
            (tone_window[l], pool[l]),
            "tone, level {level}"
        );
        assert_eq!(Level::new(level).ridges.len(), ridges[l], "level {level}");
    }
}

/// The side channel of a 32-bit stream is analysed as i64. When its values are small enough
/// for an exact covariance (wasted bits removed, or quiet audio), its LPC orders are scored and
/// coded by the same table as any other channel's.
#[test]
fn a_wide_side_channel_that_fits_is_scored_like_any_other() {
    let music: Vec<i64> = (0..4096)
        .map(|t| {
            let t = f64::from(t);
            let tone =
                (t * 0.06).sin() * 0.4 + (t * 0.0007).sin() * 0.25 + (t * 0.0213).sin() * 0.1;
            (tone * 20_000.0) as i64
        })
        .zip(pseudo(10, 4096, 5))
        .map(|(a, b)| a + i64::from(b))
        .collect();
    for (level, coded) in [(5u8, 2usize), (7, 2), (8, 3)] {
        let mut plan = Plan::new(4096);
        let mut a = Analyzer::new(Level::new(level), 32, true);
        a.analyze(&music, 33, &mut plan);
        assert_eq!(a.window_codings, coded, "level {level}");
    }
}

fn threaded(level: u8, threads: usize) -> Encoder {
    Encoder::new(EncoderConfig {
        sample_rate: 44_100,
        channels: 2,
        bits_per_sample: 16,
        level,
        threads,
    })
    .unwrap()
}

#[test]
fn a_panic_in_a_worker_is_an_error_from_then_on() {
    let samples = pseudo(16, 4096 * 2 * 12, 3);
    for threads in [2, 4] {
        let mut enc = threaded(5, threads);
        enc.panic_at = Some(3);
        let mut out = Vec::new();
        let first = enc
            .write(&samples, &mut out)
            .and_then(|()| enc.flush(&mut out));
        let Err(Error::Internal(what)) = first else {
            panic!("{threads} threads: {first:?}")
        };
        assert!(
            what.contains("panicked") && what.contains("frame 3"),
            "{what}"
        );
        // Only the frames before the one that failed were written.
        let mut before = Vec::new();
        threaded(5, 1)
            .write(&samples[..4096 * 2 * 3], &mut before)
            .unwrap();
        assert_eq!(out, before, "{threads} threads");
        assert!(matches!(
            enc.write(&samples[..2], &mut out),
            Err(Error::Internal(_))
        ));
        assert!(matches!(enc.flush(&mut out), Err(Error::Internal(_))));
    }
}

#[test]
fn a_stream_of_one_block_starts_no_thread() {
    let mut enc = threaded(5, 8);
    let mut out = Vec::new();
    enc.write(&pseudo(16, 4096 * 2 - 6, 1), &mut out).unwrap();
    enc.flush(&mut out).unwrap();
    assert_eq!(enc.pool.as_ref().map_or(0, |p| p.workers()), 0);
    assert!(enc.md5_stage.is_none());
    assert!(!out.is_empty());
}

#[test]
fn a_long_stream_codes_and_hashes_on_threads_of_its_own() {
    let mut enc = threaded(5, 4);
    let mut out = Vec::new();
    enc.write(&pseudo(16, 4096 * 2 * 20, 2), &mut out).unwrap();
    assert!(enc.pool.as_ref().is_some_and(|p| p.workers() >= 1));
    assert!(enc.md5_stage.is_some());
    enc.flush(&mut out).unwrap();
    let info = enc.finish();
    assert_eq!(info.total_samples, 4096 * 20);
    assert!(enc.md5_stage.is_none(), "finish joins the MD5 thread");
}

/// Auto leaves three cores for the rest of the pipeline (the caller's reading and writing, the
/// MD5 thread, the file I/O): measured on 8 performance and 2 efficiency cores, wall time stops
/// falling at 7 threads and only CPU time grows past it.
#[test]
fn auto_threads_leave_room_for_the_serial_stages() {
    let table = [
        (1, 1),
        (2, 2),
        (3, 2),
        (4, 2),
        (6, 3),
        (8, 5),
        (10, 7),
        (11, 8),
        (64, 8),
    ];
    for (cores, threads) in table {
        assert_eq!(auto_threads(cores), threads, "{cores} cores");
    }
}

/// Two blocks, the second short, as a tenth of a second of 44.1 kHz audio at level 5: the
/// flush codes the last block on the caller rather than start threads for it.
#[test]
fn a_stream_of_two_blocks_starts_no_thread() {
    let mut enc = threaded(5, 8);
    let mut out = Vec::new();
    enc.write(&pseudo(16, (4096 + 314) * 2, 1), &mut out)
        .unwrap();
    enc.flush(&mut out).unwrap();
    assert_eq!(enc.pool.as_ref().map_or(0, |p| p.workers()), 0);
    assert!(enc.md5_stage.is_none());
    let info = enc.finish();
    assert_eq!(info.total_samples, 4096 + 314);
}
