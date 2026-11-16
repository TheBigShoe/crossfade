use super::*;
use crate::celt::ec::Ec;
use crate::range::RangeDecoder;

fn close(a: &[f32], b: &[f32]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
}

#[test]
fn haar_is_its_own_inverse() {
    let orig: Vec<f32> = (0..16).map(|i| (i as f32 * 0.37).sin()).collect();
    for stride in [1, 2, 4] {
        let mut x = orig.clone();
        haar1(&mut x, 16 / stride, stride);
        assert!(!close(&x, &orig));
        haar1(&mut x, 16 / stride, stride);
        assert!(close(&x, &orig), "stride {stride}");
    }
}

#[test]
fn interleaving_undoes_deinterleaving() {
    let orig: Vec<f32> = (0..32).map(|i| i as f32).collect();
    for (n0, stride) in [(16, 2), (8, 4), (4, 8), (2, 16)] {
        for hadamard in [false, true] {
            let mut x = orig.clone();
            deinterleave_hadamard(&mut x, n0, stride, hadamard);
            interleave_hadamard(&mut x, n0, stride, hadamard);
            assert_eq!(x, orig);
        }
    }
    // Plain deinterleaving groups each block's bins.
    let mut x: Vec<f32> = (0..8).map(|i| i as f32).collect();
    deinterleave_hadamard(&mut x, 4, 2, false);
    assert_eq!(x, [0.0, 2.0, 4.0, 6.0, 1.0, 3.0, 5.0, 7.0]);
}

#[test]
fn qn_follows_the_bits() {
    // Too few bits: no angle.
    assert_eq!(compute_qn(8, 10, 0, 0, false), 1);
    // Plenty of bits: qb caps at 8 whole bits, exp2_table8[0] >> 6 = 256.
    assert_eq!(compute_qn(4, 1000, 100, 0, false), 256);
    assert_eq!(compute_qn(2, 32 + 32 + 8 * 4, 0, 32, true), 16);
}

#[test]
fn stereo_merge_restores_left_and_right() {
    // mid = cos(t), side = sin(t) of unit mid and side vectors.
    let t = 0.3f32;
    let m = [0.6f32, 0.8];
    let s = [0.8f32, -0.6];
    let mut x = m;
    let mut y = [s[0] * t.sin(), s[1] * t.sin()];
    stereo_merge(&mut x, &mut y, t.cos());
    let l: Vec<f32> = (0..2).map(|i| t.cos() * m[i] - t.sin() * s[i]).collect();
    let r: Vec<f32> = (0..2).map(|i| t.cos() * m[i] + t.sin() * s[i]).collect();
    let nl = l.iter().map(|v| v * v).sum::<f32>().sqrt();
    let nr = r.iter().map(|v| v * v).sum::<f32>().sqrt();
    assert!(close(&x, &[l[0] / nl, l[1] / nl]), "{x:?}");
    assert!(close(&y, &[r[0] / nr, r[1] / nr]), "{y:?}");
}

#[test]
fn denormalise_scales_each_band_and_clears_the_rest() {
    let x = vec![1.0f32; 120];
    let mut band_e = [0.0f32; NB_EBANDS];
    for (i, e) in band_e.iter_mut().enumerate() {
        *e = i as f32;
    }
    let mut freq = vec![9.0f32; 120];
    denormalise_bands(&x, &mut freq, &band_e, 13, 1);
    assert_eq!(freq[0], 0.0);
    assert_eq!(freq[9], 8.0); // bin 9 is in band 8 (8..10)
    assert_eq!(freq[19], 12.0);
    assert!(freq[20..].iter().all(|&v| v == 0.0));
}

#[test]
fn bands_without_bits_fold_or_fill_with_unit_norm() {
    // No bits at all: every band is noise or folded, each of unit norm, and nothing is read.
    let frame = [0u8; 4];
    let mut dec = RangeDecoder::new(&frame);
    let mut ec = Ec::new(&mut dec);
    ec.pretend_read_to(32);
    let pulses = [0; NB_EBANDS];
    let tf_res = [0; NB_EBANDS];
    let a = AllBands {
        start: 0,
        end: 21,
        pulses: &pulses,
        short_blocks: false,
        spread: 2,
        dual_stereo: false,
        intensity: 0,
        tf_res: &tf_res,
        total_bits: 32 * 8,
        balance: 0,
        lm: 3,
        coded_bands: 21,
        band_e: &[0.0; 2 * NB_EBANDS],
        disable_inv: false,
        theta_rdo: false,
    };
    let mut x = vec![0.0f32; 960];
    let mut masks = [0u8; 2 * NB_EBANDS];
    let mut seed = 12345;
    let rng = dec_rng(&ec);
    quant_all_bands(&a, &mut x, None, &mut masks, &mut ec, &mut seed);
    assert_eq!(dec_rng(&ec), rng);
    assert_ne!(seed, 12345);
    for i in 0..21 {
        let band = &x[8 * eb(i)..8 * eb(i + 1)];
        let norm = band.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "band {i}: {norm}");
    }
}

fn dec_rng(ec: &Ec<'_, '_>) -> u32 {
    ec.rng()
}

#[test]
fn hybrid_folding_repeats_the_first_band_into_the_second() {
    // RFC 8251 section 9: with start band 17 and no bits, band 17 is noise and band 18
    // (12 bins per block, wider than band 17's 8) folds from band 17, its last third repeating
    // the second half of band 17, instead of falling back to the LCG.
    for lm in 0..4 {
        let m = 1 << lm;
        let frame = [0u8; 4];
        let mut dec = RangeDecoder::new(&frame);
        let mut ec = Ec::new(&mut dec);
        ec.pretend_read_to(32);
        let pulses = [0; NB_EBANDS];
        let tf_res = [0; NB_EBANDS];
        let a = AllBands {
            start: 17,
            end: 21,
            pulses: &pulses,
            short_blocks: false,
            spread: 2,
            dual_stereo: false,
            intensity: 0,
            tf_res: &tf_res,
            total_bits: 32 * 8,
            balance: 0,
            lm,
            coded_bands: 21,
            band_e: &[0.0; 2 * NB_EBANDS],
            disable_inv: false,
            theta_rdo: false,
        };
        let mut x = vec![0.0f32; 120 * m];
        let mut masks = [0u8; 2 * NB_EBANDS];
        let mut seed = 777;
        quant_all_bands(&a, &mut x, None, &mut masks, &mut ec, &mut seed);
        let b17 = &x[40 * m..48 * m];
        let b18 = &x[48 * m..60 * m];
        // The fold source: band 17, then band 17's second half again.
        let source: Vec<f32> = b17.iter().chain(&b17[4 * m..]).copied().collect();
        let dot: f32 = source.iter().zip(b18).map(|(a, b)| a * b).sum();
        let ns = source.iter().map(|v| v * v).sum::<f32>().sqrt();
        let nb = b18.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!(
            dot / (ns * nb) > 0.99,
            "lm {lm}: correlation {}",
            dot / (ns * nb)
        );
    }
}

/// Unit-norm bands for `c` channels of `n` bins: noise with a few strong partials, the second
/// channel partly the first.
fn shapes(c: usize, lm: usize, seed: u32) -> Vec<f32> {
    let n = 120 << lm;
    let mut s = seed;
    let mut x = vec![0.0f32; c * n];
    for (j, v) in x.iter_mut().enumerate() {
        s = lcg_rand(s);
        let noise = ((s >> 9) as f32 / (1u32 << 23) as f32) - 0.5;
        *v = if j % 7 == 3 { 5.0 * noise } else { noise };
    }
    if c == 2 {
        for j in 0..n {
            x[n + j] = 0.6 * x[j] + 0.4 * x[n + j];
        }
    }
    for ch in 0..c {
        for i in 0..NB_EBANDS {
            let band = &mut x[ch * n + (eb(i) << lm)..ch * n + (eb(i + 1) << lm)];
            let g = 1.0 / band.iter().map(|v| v * v).sum::<f32>().sqrt();
            band.iter_mut().for_each(|v| *v *= g);
        }
        // Above the last band (20 kHz) nothing is coded.
        x[ch * n + (eb(NB_EBANDS) << lm)..(ch + 1) * n].fill(0.0);
    }
    x
}

/// One frame's shapes coded into `bytes` and decoded: the original, the encoder's
/// resynthesis and the decoder's output, with the masks and seeds both ended on.
struct ShapeTrip {
    orig: Vec<f32>,
    coded: Vec<f32>,
    decoded: Vec<f32>,
}

#[allow(clippy::too_many_arguments)]
fn shape_round_trip(
    c: usize,
    lm: usize,
    bytes: usize,
    short_blocks: bool,
    tf: i32,
    spread: i32,
    intensity: usize,
    dual_stereo: bool,
    theta_rdo: bool,
) -> ShapeTrip {
    use crate::celt::ec::Enc;
    use crate::celt::rate::{AllocHint, compute_allocation, init_caps};
    use crate::range_encoder::RangeEncoder;
    let n = 120 << lm;
    let (start, end) = (0, NB_EBANDS);
    let orig = shapes(c, lm, (c * 1000 + lm * 100 + bytes) as u32);
    let cap = init_caps(lm, c);
    let offsets = [0; NB_EBANDS];
    let tf_res = [tf; NB_EBANDS];
    let mut band_e = [0.0f32; 2 * NB_EBANDS];
    for (i, e) in band_e.iter_mut().enumerate() {
        // The right channel quieter, as intensity stereo weighs them.
        *e = if i < NB_EBANDS { 1.0 } else { 0.5 };
    }
    let hint = AllocHint {
        intensity,
        dual_stereo,
        prev: 0,
        signal_bandwidth: 20,
    };
    let total_bits = bytes as i32 * (8 << BITRES);

    let mut coded = orig.clone();
    let mut renc = RangeEncoder::new(bytes);
    let mut enc = Enc::new(&mut renc);
    let bits = total_bits - enc.tell_frac() - 1;
    let alloc = compute_allocation(start, end, &offsets, &cap, 5, bits, c, lm, &mut enc, &hint);
    let all = |alloc: &'_ crate::celt::rate::Allocation| AllBands {
        start,
        end,
        pulses: &[0; NB_EBANDS],
        short_blocks,
        spread,
        dual_stereo: alloc.dual_stereo,
        intensity: alloc.intensity,
        tf_res: &[0; NB_EBANDS],
        total_bits,
        balance: alloc.balance,
        lm,
        coded_bands: alloc.coded_bands,
        band_e: &[0.0; 2 * NB_EBANDS],
        disable_inv: false,
        theta_rdo,
    };
    let mut enc_masks = [0u8; 2 * NB_EBANDS];
    let mut enc_seed = 77;
    {
        let a = AllBands {
            pulses: &alloc.pulses,
            tf_res: &tf_res,
            band_e: &band_e,
            ..all(&alloc)
        };
        let (xa, ya) = coded.split_at_mut(n);
        let y = (c == 2).then_some(ya);
        quant_all_bands(&a, xa, y, &mut enc_masks, &mut enc, &mut enc_seed);
    }
    let rng = renc.rng();
    let frame = renc.done().expect("the shapes stay in their budget");

    let mut decoded = vec![0.0f32; c * n];
    let mut dec = RangeDecoder::new(&frame);
    let mut ec = Ec::new(&mut dec);
    let bits = total_bits - ec.tell_frac() - 1;
    let read = compute_allocation(
        start,
        end,
        &offsets,
        &cap,
        5,
        bits,
        c,
        lm,
        &mut ec,
        &AllocHint::default(),
    );
    assert_eq!(read, alloc);
    let mut dec_masks = [0u8; 2 * NB_EBANDS];
    let mut dec_seed = 77;
    {
        let a = AllBands {
            pulses: &read.pulses,
            tf_res: &tf_res,
            ..all(&read)
        };
        let (xa, ya) = decoded.split_at_mut(n);
        let y = (c == 2).then_some(ya);
        quant_all_bands(&a, xa, y, &mut dec_masks, &mut ec, &mut dec_seed);
    }
    let what = format!("c {c} lm {lm} {bytes} bytes short {short_blocks} tf {tf} spread {spread}");
    assert_eq!(dec.rng(), rng, "{what}");
    assert!(!dec.error(), "{what}");
    assert_eq!(dec_masks, enc_masks, "{what}");
    assert_eq!(dec_seed, enc_seed, "{what}");
    ShapeTrip {
        orig,
        coded,
        decoded,
    }
}

/// The mean over the bands of the cosine between two sets of unit-norm bands.
fn likeness(a: &[f32], b: &[f32], lm: usize) -> f32 {
    let n = 120 << lm;
    let c = a.len() / n;
    let mut sum = 0.0;
    for ch in 0..c {
        for i in 0..NB_EBANDS {
            let r = ch * n + (eb(i) << lm)..ch * n + (eb(i + 1) << lm);
            sum += a[r.clone()]
                .iter()
                .zip(&b[r])
                .map(|(x, y)| x * y)
                .sum::<f32>();
        }
    }
    sum / (c * NB_EBANDS) as f32
}

#[test]
fn the_decoder_rebuilds_the_shapes_the_encoder_resynthesised_bit_for_bit() {
    // Mono and every stereo coding (mid/side, intensity from band 8, dual), long and short
    // blocks, the time-frequency changes both ways, every spread, starved to generous.
    for lm in 0..4 {
        for bytes in [6usize, 25, 80, 300] {
            for (short_blocks, tf) in [(false, 0), (true, 0), (false, -1), (true, 1), (false, -2)] {
                if (short_blocks || tf != 0) && lm == 0 {
                    continue;
                }
                let tf = if tf == -2 { -(lm as i32).min(2) } else { tf };
                for spread in [0, 2, 3] {
                    let t =
                        shape_round_trip(1, lm, bytes, short_blocks, tf, spread, 0, false, false);
                    assert_eq!(t.decoded, t.coded, "mono lm {lm} {bytes} bytes tf {tf}");
                    for (intensity, dual) in [(21, false), (8, false), (0, false), (21, true)] {
                        let t = shape_round_trip(
                            2,
                            lm,
                            bytes,
                            short_blocks,
                            tf,
                            spread,
                            intensity,
                            dual,
                            // The angle search is tried on every other case.
                            (bytes + lm + intensity) % 2 == 0,
                        );
                        assert_eq!(
                            t.decoded, t.coded,
                            "stereo lm {lm} {bytes} bytes tf {tf} intensity {intensity} dual {dual}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn more_bits_bring_the_coded_shapes_closer_to_the_originals() {
    for c in 1..=2 {
        let like: Vec<f32> = [10usize, 40, 160, 600]
            .iter()
            .map(|&bytes| {
                let t = shape_round_trip(c, 3, bytes, false, 0, 2, 21, false, false);
                likeness(&t.orig, &t.decoded, 3)
            })
            .collect();
        assert!(like.windows(2).all(|w| w[1] > w[0]), "c {c}: {like:?}");
        assert!(like[3] > 0.97, "c {c}: {like:?}");
        assert!(like[0] < 0.5, "c {c}: {like:?}");
    }
}

#[test]
fn searching_the_stereo_angles_brings_the_coded_shapes_closer() {
    // The same frames coded with each band's angle rounded both ways and the nearer kept:
    // never much worse, and better on the whole.
    let mut gain = 0.0f32;
    let mut cases = 0;
    for lm in 1..4 {
        for bytes in [30usize, 60, 120, 240] {
            let plain = shape_round_trip(2, lm, bytes, false, 0, 2, 21, false, false);
            let searched = shape_round_trip(2, lm, bytes, false, 0, 2, 21, false, true);
            assert_eq!(searched.decoded, searched.coded, "lm {lm} {bytes} bytes");
            let (a, b) = (
                likeness(&plain.orig, &plain.decoded, lm),
                likeness(&searched.orig, &searched.decoded, lm),
            );
            assert!(b > a - 0.01, "lm {lm} {bytes} bytes: {b} against {a}");
            gain += b - a;
            cases += 1;
        }
    }
    assert!(
        gain / cases as f32 > 0.001,
        "mean gain {}",
        gain / cases as f32
    );
}
