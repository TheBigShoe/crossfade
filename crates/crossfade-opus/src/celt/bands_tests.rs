use super::*;
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
