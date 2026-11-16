use super::*;
use crate::celt::laplace::tests::laplace_symbol;
use crate::celt::test_util::encode;
use crate::range::RangeDecoder;
use crate::testing::range_encoder::Op;

#[test]
fn coarse_energy_predicts_from_the_last_frame_and_the_band_below() {
    // Inter, LM 3, mono, two bands, deltas +2 then -1, from last energies 1.0 and -12.0
    // (clamped to -9 first).
    let model = &E_PROB_MODEL[3][0];
    let mut ops = vec![];
    for (i, q) in [(0usize, 2), (1, -1)] {
        let (fl, fh, _) = laplace_symbol(
            q,
            u32::from(model[2 * i]) << 7,
            u32::from(model[2 * i + 1]) << 6,
        );
        ops.push(Op::Bin { fl, fh, bits: 15 });
    }
    ops.push(Op::Bits { val: 0, bits: 25 });
    ops.push(Op::Bits { val: 0, bits: 25 });
    let frame = encode(&ops);
    let mut dec = RangeDecoder::new(&frame);
    let mut ec = Ec::new(&mut dec);
    let mut old = [0.0; 2 * NB_EBANDS];
    old[0] = 1.0;
    old[1] = -12.0;
    unquant_coarse_energy(&mut old, 0, 2, false, &mut ec, 1, 3);
    let (coef, beta) = (PRED_COEF[3], BETA_COEF[3]);
    let e0 = coef * 1.0 + 0.0 + 2.0;
    let prev = 2.0 - beta * 2.0;
    let e1 = coef * -9.0 + prev - 1.0;
    assert_eq!(old[0], e0);
    assert_eq!(old[1], e1);
}

#[test]
fn out_of_bits_the_delta_is_minus_one() {
    // A frame with no bits left: every band falls by one step plus the prediction.
    let frame = [0u8; 2];
    let mut dec = RangeDecoder::new(&frame);
    let mut ec = Ec::new(&mut dec);
    ec.pretend_read_to(16);
    let mut old = [0.0; 2 * NB_EBANDS];
    unquant_coarse_energy(&mut old, 0, 3, true, &mut ec, 2, 0);
    assert_eq!(old[0], -1.0);
    assert_eq!(old[NB_EBANDS], -1.0);
    assert_eq!(old[1], -1.0 + (-1.0 + BETA_INTRA));
}

#[test]
fn fine_and_final_bits_refine_by_halves() {
    // Band 0 gets 2 fine bits (value 3: +0.375) then a final bit (1: +0.0625).
    let frame = encode(&[Op::Bits { val: 3, bits: 2 }, Op::Bits { val: 1, bits: 1 }]);
    let mut dec = RangeDecoder::new(&frame);
    let mut ec = Ec::new(&mut dec);
    let mut old = [0.0; 2 * NB_EBANDS];
    let mut fine = [0; NB_EBANDS];
    fine[0] = 2;
    unquant_fine_energy(&mut old, 0, 1, &fine, &mut ec, 1);
    assert_eq!(old[0], 0.375);
    unquant_energy_finalise(&mut old, 0, 1, &fine, &[0; NB_EBANDS], 1, &mut ec, 1);
    assert_eq!(old[0], 0.4375);
}

#[test]
fn amplitudes_are_capped_and_zero_outside_the_coded_bands() {
    let mut old = [0.0; 2 * NB_EBANDS];
    old[5] = 1000.0;
    let mut e = [1.0; 2 * NB_EBANDS];
    log2_amp(&mut e, &old, 1, 20, 1);
    assert_eq!(e[0], 0.0);
    assert_eq!(e[20], 0.0);
    assert_eq!(e[5], exp2(32.0));
    assert_eq!(e[1], exp2(E_MEANS[1]));
}

/// Log energies like a real spectrum's: falling with frequency, a different level per frame
/// and channel.
fn energies(frame: usize, c: usize) -> [f32; 2 * NB_EBANDS] {
    let mut e = [0.0f32; 2 * NB_EBANDS];
    for ch in 0..c {
        for i in 0..NB_EBANDS {
            let wobble = ((i * 7 + frame * 13 + ch * 5) % 11) as f32 * 0.37;
            e[ch * NB_EBANDS + i] = 6.0 - 0.45 * i as f32 + wobble - 1.5 * frame as f32;
        }
    }
    e
}

/// Codes `frames` frames of [`energies`] into `bytes` each and decodes them: the encoder's
/// state after each frame, the decoder's, and the targets.
#[allow(clippy::type_complexity)]
fn energy_round_trip(
    c: usize,
    lm: usize,
    bytes: usize,
    two_pass: bool,
    force_intra: bool,
    fine: i32,
) -> Vec<(
    [f32; 2 * NB_EBANDS],
    [f32; 2 * NB_EBANDS],
    [f32; 2 * NB_EBANDS],
    bool,
)> {
    use crate::celt::ec::Enc;
    use crate::range_encoder::RangeEncoder;
    let (start, end) = (0, NB_EBANDS);
    let fine_quant = [fine; NB_EBANDS];
    let mut fine_priority = [0; NB_EBANDS];
    fine_priority.iter_mut().step_by(2).for_each(|p| *p = 1);
    let mut enc_old = [0.0f32; 2 * NB_EBANDS];
    let mut dec_old = [0.0f32; 2 * NB_EBANDS];
    let mut delayed_intra = 1.0f32;
    let mut out = Vec::new();
    for frame in 0..3 {
        let target = energies(frame, c);
        let mut error = [0.0f32; 2 * NB_EBANDS];
        let mut renc = RangeEncoder::new(bytes);
        let mut enc = Enc::new(&mut renc);
        let p = CoarseEnergy {
            start,
            end,
            eff_end: end,
            c,
            lm,
            nb_available_bytes: bytes as i32,
            force_intra,
            two_pass,
            loss_rate: 0,
            lfe: false,
        };
        quant_coarse_energy(
            &p,
            &target,
            &mut enc_old,
            (bytes * 8) as u32,
            &mut error,
            &mut enc,
            &mut delayed_intra,
        );
        quant_fine_energy(
            &mut enc_old,
            &mut error,
            start,
            end,
            &fine_quant,
            &mut enc,
            c,
        );
        let left = (bytes * 8) as i32 - enc.tell();
        quant_energy_finalise(
            &mut enc_old,
            &mut error,
            start,
            end,
            &fine_quant,
            &fine_priority,
            left,
            &mut enc,
            c,
        );
        let rng = renc.rng();
        let frame_bytes = renc.done().expect("the budget is respected");

        let mut dec = RangeDecoder::new(&frame_bytes);
        let mut ec = Ec::new(&mut dec);
        let intra = ec.tell() + 3 <= (bytes * 8) as i32 && ec.bit_logp(3);
        unquant_coarse_energy(&mut dec_old, start, end, intra, &mut ec, c, lm);
        unquant_fine_energy(&mut dec_old, start, end, &fine_quant, &mut ec, c);
        let left = (bytes * 8) as i32 - ec.tell();
        unquant_energy_finalise(
            &mut dec_old,
            start,
            end,
            &fine_quant,
            &fine_priority,
            left,
            &mut ec,
            c,
        );
        assert_eq!(
            dec.rng(),
            rng,
            "frame {frame}: the same symbols, the same range"
        );
        out.push((enc_old, dec_old, target, intra));
    }
    out
}

#[test]
fn the_decoder_reads_the_energies_the_encoder_wrote() {
    for c in 1..=2 {
        for lm in 0..4 {
            for (two_pass, force_intra) in [(true, false), (false, false), (false, true)] {
                let frames = energy_round_trip(c, lm, 200, two_pass, force_intra, 2);
                for (f, (enc_old, dec_old, target, intra)) in frames.iter().enumerate() {
                    let what = format!("c {c} lm {lm} two_pass {two_pass} frame {f}");
                    assert_eq!(enc_old, dec_old, "{what}");
                    assert!(*intra || !force_intra, "{what}");
                    for i in 0..c * NB_EBANDS {
                        // Coarse steps of 1 (6 dB), two fine bits and one final bit: 1/16.
                        let err = (enc_old[i] - target[i]).abs();
                        assert!(err <= 0.0626, "{what} band {i}: off by {err}");
                    }
                }
            }
        }
    }
}

#[test]
fn energies_stay_in_step_when_the_frame_is_too_small_for_them() {
    // 3 to 12 bytes for 21 or 42 bands: the coarse coder falls back to its small alphabets
    // and then to nothing, exactly as the decoder does.
    for c in 1..=2 {
        for bytes in [2, 3, 5, 8, 12] {
            let frames = energy_round_trip(c, 3, bytes, true, false, 0);
            for (f, (enc_old, dec_old, _, _)) in frames.iter().enumerate() {
                assert_eq!(enc_old, dec_old, "c {c} {bytes} bytes frame {f}");
            }
        }
    }
}

#[test]
fn log_energies_are_log2_of_the_amplitudes_less_the_mean() {
    let mut band_e = [0.0f32; 2 * NB_EBANDS];
    for (i, e) in band_e.iter_mut().enumerate() {
        *e = 0.25 * (1 << (i % 12)) as f32;
    }
    let mut log_e = [9.0f32; 2 * NB_EBANDS];
    amp2_log2(&band_e, &mut log_e, 19, NB_EBANDS, 2);
    for ch in 0..2 {
        for i in 0..19 {
            let want = ((ch * NB_EBANDS + i) % 12) as f32 - 2.0 - E_MEANS[i];
            assert!((log_e[ch * NB_EBANDS + i] - want).abs() < 1e-5, "band {i}");
        }
        // Bands the mode has but the stream's rate does not: far below anything.
        assert_eq!(log_e[ch * NB_EBANDS + 19..(ch + 1) * NB_EBANDS], [-14.0; 2]);
    }
    // And back, through the decoder's log2_amp.
    let mut back = [0.0f32; 2 * NB_EBANDS];
    log2_amp(&mut back, &log_e, 0, 19, 2);
    for i in 0..19 {
        assert!((back[i] / band_e[i] - 1.0).abs() < 1e-5, "band {i}");
    }
}
