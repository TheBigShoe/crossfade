use super::resampler::Resampler;
use super::tables::*;

fn assert_icdf(name: &str, t: &[u8]) {
    assert_eq!(t.last(), Some(&0), "{name} ends in 0");
    assert!(t.windows(2).all(|p| p[0] >= p[1]), "{name} falls");
}

#[test]
fn icdf_tables_fall_and_end_in_zero() {
    let tables: [(&str, &[u8]); 25] = [
        ("GAIN_ICDF[0]", &GAIN_ICDF[0]),
        ("GAIN_ICDF[1]", &GAIN_ICDF[1]),
        ("GAIN_ICDF[2]", &GAIN_ICDF[2]),
        ("DELTA_GAIN_ICDF", &DELTA_GAIN_ICDF),
        ("LTP_PER_INDEX_ICDF", &LTP_PER_INDEX_ICDF),
        ("LTP_GAIN_ICDF_0", &LTP_GAIN_ICDF_0),
        ("LTP_GAIN_ICDF_1", &LTP_GAIN_ICDF_1),
        ("LTP_GAIN_ICDF_2", &LTP_GAIN_ICDF_2),
        ("STEREO_PRED_JOINT_ICDF", &STEREO_PRED_JOINT_ICDF),
        ("STEREO_ONLY_CODE_MID_ICDF", &STEREO_ONLY_CODE_MID_ICDF),
        ("LBRR_FLAGS_2_ICDF", &LBRR_FLAGS_2_ICDF),
        ("LBRR_FLAGS_3_ICDF", &LBRR_FLAGS_3_ICDF),
        ("LSB_ICDF", &LSB_ICDF),
        ("LTPSCALE_ICDF", &LTPSCALE_ICDF),
        ("TYPE_OFFSET_VAD_ICDF", &TYPE_OFFSET_VAD_ICDF),
        ("TYPE_OFFSET_NO_VAD_ICDF", &TYPE_OFFSET_NO_VAD_ICDF),
        (
            "NLSF_INTERPOLATION_FACTOR_ICDF",
            &NLSF_INTERPOLATION_FACTOR_ICDF,
        ),
        ("NLSF_EXT_ICDF", &NLSF_EXT_ICDF),
        ("PITCH_LAG_ICDF", &PITCH_LAG_ICDF),
        ("PITCH_DELTA_ICDF", &PITCH_DELTA_ICDF),
        ("PITCH_CONTOUR_ICDF", &PITCH_CONTOUR_ICDF),
        ("PITCH_CONTOUR_NB_ICDF", &PITCH_CONTOUR_NB_ICDF),
        ("PITCH_CONTOUR_10_MS_ICDF", &PITCH_CONTOUR_10_MS_ICDF),
        ("PITCH_CONTOUR_10_MS_NB_ICDF", &PITCH_CONTOUR_10_MS_NB_ICDF),
        ("RATE_LEVELS_ICDF[0]", &RATE_LEVELS_ICDF[0]),
    ];
    for (name, t) in tables {
        assert_icdf(name, t);
    }
    assert_icdf("RATE_LEVELS_ICDF[1]", &RATE_LEVELS_ICDF[1]);
    for (n, row) in PULSES_PER_BLOCK_ICDF.iter().enumerate() {
        assert_icdf(&format!("PULSES_PER_BLOCK_ICDF[{n}]"), row);
    }
    for (k, t) in [3, 4, 5, 6, 8].into_iter().zip(UNIFORM_TABLES) {
        assert_eq!(t.len(), k);
        assert_icdf("UNIFORM", t);
    }
    // The shell coder's 16 sub-tables: p + 1 symbols for each pulse count p.
    for table in [
        &SHELL_CODE_TABLE0,
        &SHELL_CODE_TABLE1,
        &SHELL_CODE_TABLE2,
        &SHELL_CODE_TABLE3,
    ] {
        for p in 1..=16usize {
            let at = usize::from(SHELL_CODE_TABLE_OFFSETS[p]);
            assert_eq!(at, (1..p).map(|q| q + 1).sum::<usize>());
            assert_icdf("SHELL", &table[at..at + p + 1]);
        }
    }
    assert_eq!(SIGN_ICDF.len(), 42);
}

const UNIFORM_TABLES: [&[u8]; 5] = [
    &UNIFORM3_ICDF,
    &UNIFORM4_ICDF,
    &UNIFORM5_ICDF,
    &UNIFORM6_ICDF,
    &UNIFORM8_ICDF,
];

#[test]
fn ltp_and_pitch_tables_have_their_rfc_shapes() {
    assert_eq!(LTP_GAIN_VQ_0.len(), LTP_GAIN_ICDF_0.len());
    assert_eq!(LTP_GAIN_VQ_1.len(), LTP_GAIN_ICDF_1.len());
    assert_eq!(LTP_GAIN_VQ_2.len(), LTP_GAIN_ICDF_2.len());
    assert_eq!(CB_LAGS_STAGE2[0].len(), PITCH_CONTOUR_NB_ICDF.len());
    assert_eq!(CB_LAGS_STAGE3[0].len(), PITCH_CONTOUR_ICDF.len());
    assert_eq!(
        CB_LAGS_STAGE2_10_MS[0].len(),
        PITCH_CONTOUR_10_MS_NB_ICDF.len()
    );
    assert_eq!(
        CB_LAGS_STAGE3_10_MS[0].len(),
        PITCH_CONTOUR_10_MS_ICDF.len()
    );
    assert_eq!(STEREO_PRED_QUANT_Q13.len(), 16);
    assert_eq!(STEREO_PRED_JOINT_ICDF.len(), 25);
}

#[test]
fn the_resampler_keeps_dc_and_delays_as_libopus() {
    for (rate, delay) in [(8000, 0), (12000, 4), (16000, 7)] {
        let mut r = Resampler::new(rate);
        assert_eq!(r.input_delay(), delay);
        let khz = rate as usize / 1000;
        let input = vec![10_000i16; 20 * khz];
        let mut out = vec![0i16; 960];
        for _ in 0..5 {
            r.process(&mut out, &input);
        }
        for &s in &out {
            assert!((i32::from(s) - 10_000).abs() < 100, "{rate}: {s}");
        }
    }
}
