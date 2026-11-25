use super::*;
use crate::celt::bands::{AllBands, quant_all_bands};
use crate::celt::ec::Ec;
use crate::celt::rate::{AllocEncoder, compute_allocation, init_caps};
use crate::range::RangeDecoder;

/// A frame's normalised shapes for `c` channels of `n` bins each (unit norm per band), and
/// the band amplitudes they came from.
fn shapes(n: usize, lm: usize, c: usize, seed: u32) -> (Vec<f32>, [f32; 2 * NB_EBANDS]) {
    let m = 1 << lm;
    let mut x = vec![0.0f32; c * n];
    let mut band_e = [0.0f32; 2 * NB_EBANDS];
    let mut s = seed;
    for ch in 0..c {
        for i in 0..NB_EBANDS {
            let lo = m * EBANDS[i] as usize;
            let hi = m * EBANDS[i + 1] as usize;
            let mut e = 0.0f32;
            for j in lo..hi {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let tone = libm::sinf(j as f32 * (0.3 + 0.1 * ch as f32) + seed as f32);
                let noise = ((s >> 9) as f32 / (1 << 23) as f32) - 0.5;
                // The second channel is mostly the first: stereo correlation.
                let v = if ch == 1 {
                    0.8 * x[j] + 0.3 * noise
                } else {
                    tone + noise
                };
                x[ch * n + j] = v;
                e += v * v;
            }
            let e = e.sqrt() + 1e-9;
            band_e[ch * NB_EBANDS + i] = e * (1.0 + i as f32);
            for v in &mut x[ch * n + lo..ch * n + hi] {
                *v /= e;
            }
        }
    }
    (x, band_e)
}

#[test]
fn the_decoder_resynthesises_what_the_encoder_did() {
    for lm in 0..4 {
        let n = 120 << lm;
        for c in [1, 2] {
            for (storage, short, spread, dual, intensity, rdo) in [
                (160, false, 2, false, 21, true),
                (60, true, 1, false, 12, true),
                (25, false, 3, false, 6, false),
                (300, false, 2, true, 16, false),
                (900, true, 0, false, 21, true),
            ] {
                if short && lm == 0 {
                    continue;
                }
                let (start, end) = (0, if storage < 40 { 17 } else { 21 });
                let (orig, band_e) = shapes(n, lm, c, storage as u32 + lm as u32);
                let mut tf_res = [0i32; NB_EBANDS];
                for (i, t) in tf_res.iter_mut().enumerate() {
                    *t = match (short, i % 3) {
                        (_, 0) => 0,
                        (true, _) => 1,
                        (false, _) => -(lm as i32).min(1),
                    };
                }
                let what = format!("lm {lm} c {c} storage {storage} short {short}");
                let total = storage as i32 * 64 - 1;
                let cap = init_caps(lm, c);
                let mut offsets = [0; NB_EBANDS];
                offsets[3] = 16;
                let mut enc = RangeEncoder::new(storage);
                let alloc = compute_allocation(
                    start,
                    end,
                    &offsets,
                    &cap,
                    5,
                    total,
                    c,
                    lm,
                    &mut AllocEncoder {
                        enc: &mut enc,
                        intensity,
                        dual_stereo: dual && c == 2,
                        prev: 0,
                        signal_bandwidth: 20,
                    },
                );
                let mut x = orig.clone();
                let mut masks = [0u8; 2 * NB_EBANDS];
                let mut seed = 1234u32;
                {
                    let (xa, ya) = x.split_at_mut(n);
                    let bands = EncodeBands {
                        start,
                        end,
                        band_e: &band_e,
                        pulses: &alloc.pulses,
                        short_blocks: short,
                        spread,
                        dual_stereo: alloc.dual_stereo,
                        intensity: alloc.intensity,
                        tf_res: &tf_res,
                        total_bits: storage as i32 * 64,
                        balance: alloc.balance,
                        lm,
                        coded_bands: alloc.coded_bands,
                        theta_rdo: rdo,
                    };
                    let y = (c == 2).then_some(ya);
                    quant_all_bands_enc(&bands, xa, y, &mut masks, &mut enc, &mut seed);
                }
                let rng = enc.rng();
                let frame = enc.done().expect("fits");

                let mut dec = RangeDecoder::new(&frame);
                let mut ec = Ec::new(&mut dec);
                let got = compute_allocation(start, end, &offsets, &cap, 5, total, c, lm, &mut ec);
                assert_eq!(got, alloc, "{what}");
                let mut xd = vec![0.0f32; c * n];
                let mut masks_d = [0u8; 2 * NB_EBANDS];
                let mut seed_d = 1234u32;
                {
                    let (xa, ya) = xd.split_at_mut(n);
                    let all = AllBands {
                        start,
                        end,
                        pulses: &got.pulses,
                        short_blocks: short,
                        spread,
                        dual_stereo: got.dual_stereo,
                        intensity: got.intensity,
                        tf_res: &tf_res,
                        total_bits: storage as i32 * 64,
                        balance: got.balance,
                        lm,
                        coded_bands: got.coded_bands,
                    };
                    let y = (c == 2).then_some(ya);
                    quant_all_bands(&all, xa, y, &mut masks_d, &mut ec, &mut seed_d);
                }
                assert_eq!(ec.rng(), rng, "{what}");
                assert_eq!(masks, masks_d, "{what}");
                assert_eq!(seed, seed_d, "{what}");
                let m = 1 << lm;
                let coded = m * EBANDS[alloc.coded_bands] as usize;
                for ch in 0..c {
                    let row = &x[ch * n..ch * n + coded];
                    let row_d = &xd[ch * n..ch * n + coded];
                    assert_eq!(row, row_d, "{what} channel {ch}");
                }
                if storage >= 300 {
                    // Enough bits: the shapes come out near what went in.
                    let dot: f32 = (0..c * n).map(|i| x[i] * orig[i]).sum();
                    let norm: f32 = (0..c * n).map(|i| orig[i] * orig[i]).sum();
                    assert!(dot > 0.8 * norm, "{what}: {dot} of {norm}");
                }
            }
        }
    }
}
