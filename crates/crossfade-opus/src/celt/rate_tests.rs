use super::*;
use crate::celt::ec::Ec;
use crate::range::RangeDecoder;

#[test]
fn pseudo_pulses_grow_geometrically_past_eight() {
    let got: Vec<i32> = (0..20).map(get_pulses).collect();
    assert_eq!(
        got,
        [
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 18, 20, 22
        ]
    );
    assert_eq!(get_pulses(40), 128);
}

#[test]
fn bits2pulses_picks_the_nearest_cost_and_pulses2bits_inverts_it() {
    for lm in -1..=3 {
        for band in 0..NB_EBANDS {
            let c = cache(band, lm);
            let max = i32::from(c[0]);
            for q in 0..=max {
                // Exactly the cost of q pseudo-pulses gives q back (N = 1 bands cost the same
                // for any count: only their sign is coded).
                if q > 0 && c[q as usize] == c[q as usize - 1] {
                    continue;
                }
                let bits = pulses2bits(band, lm, q);
                assert_eq!(bits2pulses(band, lm, bits), q, "band {band} lm {lm} q {q}");
            }
            // Costs rise with the pulses, and asking for more never gives fewer.
            let mut last = -1;
            for bits in 0..300 {
                let q = bits2pulses(band, lm, bits);
                assert!(q >= last && q <= max);
                last = q;
            }
        }
    }
}

#[test]
fn caps_are_the_reference_formula() {
    // Band 0 at LM 0, mono: cache_caps50[0] = 224, (224 + 64) * 1 * 1 >> 2 = 72.
    assert_eq!(init_caps(0, 1)[0], 72);
    // Band 20 at LM 3, stereo: caps row 2 * 3 + 2 - 1 = 7, entry 20 is 40;
    // (40 + 64) * 2 * (22 << 3) >> 2 = 9152.
    assert_eq!(init_caps(3, 2)[20], 9152);
}

/// The allocation of `total` with every decision read from `frame`.
fn allocate(
    frame: &[u8],
    start: usize,
    end: usize,
    trim: i32,
    total: i32,
    c: usize,
    lm: usize,
) -> Allocation {
    let mut dec = RangeDecoder::new(frame);
    let mut ec = Ec::new(&mut dec);
    let offsets = [0; NB_EBANDS];
    let cap = init_caps(lm, c);
    compute_allocation(
        start,
        end,
        &offsets,
        &cap,
        trim,
        total,
        c,
        lm,
        &mut ec,
        &AllocHint::default(),
    )
}

#[test]
fn nothing_to_share_gives_nothing() {
    let a = allocate(&[0; 8], 0, 21, 5, 0, 1, 3);
    assert_eq!(a.pulses, [0; NB_EBANDS]);
    assert_eq!(a.fine_quant, [0; NB_EBANDS]);
    assert_eq!(a.balance, 0);
    assert_eq!(a.intensity, 0);
    assert!(!a.dual_stereo);
}

#[test]
fn the_allocation_never_exceeds_the_budget() {
    // Every frame size, channel count, trim and start/end the decoder uses, budgets from
    // nothing to a full 1275-byte frame, decisions read from zeros (no band skipped) and from
    // ones (every skip flag set).
    let frames: [&[u8]; 3] = [
        &[0; 64],
        &[0xFF; 64],
        &[0x5A, 0xC3, 0x99, 0x10, 0x7E, 0x01, 0xEE, 0x42],
    ];
    for frame in frames {
        for lm in 0..4 {
            for c in 1..=2 {
                for (start, end) in [(0, 13), (0, 17), (0, 19), (0, 21), (17, 19), (17, 21)] {
                    for trim in [0, 5, 10] {
                        for total in (0..1275 * 64).step_by(997) {
                            let a = allocate(frame, start, end, trim, total, c, lm);
                            let used: i32 = (start..end)
                                .map(|j| a.pulses[j] + ((c as i32 * a.fine_quant[j]) << BITRES))
                                .sum();
                            assert!(
                                used + a.balance <= total.max(0),
                                "lm {lm} c {c} {start}..{end} trim {trim} total {total}: {used} + {}",
                                a.balance
                            );
                            assert!(a.coded_bands > start && a.coded_bands <= end);
                            for j in start..end {
                                assert!(a.pulses[j] >= 0);
                                assert!((0..=8).contains(&a.fine_quant[j]));
                                assert!((0..=1).contains(&a.fine_priority[j]));
                            }
                            assert!(a.intensity <= a.coded_bands);
                            assert!(!a.dual_stereo || c == 2);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn the_decoder_reads_the_allocation_the_encoder_chose() {
    use crate::celt::ec::Enc;
    use crate::range_encoder::RangeEncoder;
    // Rates from starved to generous, both channel counts, every frame size, hybrid's start.
    for c in 1..=2 {
        for lm in 0..4 {
            for (start, end) in [(0, 21), (0, 13), (17, 21)] {
                for bytes in [3usize, 8, 20, 60, 160, 500] {
                    for (trim, wanted_intensity, dual, prev, bandwidth) in [
                        (5, 21, false, 0, 20),
                        (2, 9, true, 18, 15),
                        (9, 0, true, 21, 12),
                    ] {
                        let what =
                            format!("c {c} lm {lm} bands {start}..{end} {bytes} bytes trim {trim}");
                        let cap = init_caps(lm, c);
                        let mut offsets = [0; NB_EBANDS];
                        if bytes > 20 {
                            offsets[start + 1] = 48.min(cap[start + 1]);
                        }
                        let hint = AllocHint {
                            intensity: wanted_intensity.clamp(start, end),
                            dual_stereo: dual,
                            prev,
                            signal_bandwidth: bandwidth,
                        };
                        let mut renc = RangeEncoder::new(bytes);
                        let mut enc = Enc::new(&mut renc);
                        // Something before it, as a frame has.
                        enc.bits(0x2b, 6);
                        enc.bit_logp(false, 3);
                        let total = ((bytes as i32 * 8) << BITRES) - enc.tell_frac() - 1;
                        let chosen = compute_allocation(
                            start, end, &offsets, &cap, trim, total, c, lm, &mut enc, &hint,
                        );
                        let rng = renc.rng();
                        let frame = renc.done().expect("the allocation stays in its budget");

                        let mut dec = RangeDecoder::new(&frame);
                        let mut ec = Ec::new(&mut dec);
                        assert_eq!(ec.bits(6), 0x2b);
                        assert!(!ec.bit_logp(3));
                        let total = ((bytes as i32 * 8) << BITRES) - ec.tell_frac() - 1;
                        let read = compute_allocation(
                            start,
                            end,
                            &offsets,
                            &cap,
                            trim,
                            total,
                            c,
                            lm,
                            &mut ec,
                            &AllocHint::default(),
                        );
                        assert_eq!(read, chosen, "{what}");
                        assert_eq!(dec.rng(), rng, "{what}");
                        assert!(
                            chosen.coded_bands > start && chosen.coded_bands <= end,
                            "{what}"
                        );
                        assert!(chosen.intensity <= hint.intensity.max(start), "{what}");
                        assert!(!chosen.dual_stereo || dual, "{what}");
                    }
                }
            }
        }
    }
}

#[test]
fn the_encoder_keeps_bands_up_to_the_signal_bandwidth_when_it_can_afford_them() {
    use crate::celt::ec::Enc;
    use crate::range_encoder::RangeEncoder;
    let coded = |bytes: usize, bandwidth: i32| -> usize {
        let cap = init_caps(3, 1);
        let mut renc = RangeEncoder::new(bytes);
        let mut enc = Enc::new(&mut renc);
        let total = ((bytes as i32 * 8) << BITRES) - enc.tell_frac() - 1;
        let hint = AllocHint {
            intensity: 0,
            dual_stereo: false,
            prev: 0,
            signal_bandwidth: bandwidth,
        };
        compute_allocation(
            0,
            21,
            &[0; NB_EBANDS],
            &cap,
            5,
            total,
            1,
            3,
            &mut enc,
            &hint,
        )
        .coded_bands
    };
    // 128 kb/s mono at 20 ms: everything is coded for a full-band signal...
    assert_eq!(coded(320, 20), 21);
    // ...and nothing above band 17 for one that stops there (a skip costs a bit a band).
    assert_eq!(coded(320, 17), 18);
    // Starved, the high bands go whatever the signal has.
    assert!(coded(12, 20) < 17);
}
