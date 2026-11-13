use super::resampler::Resampler;
use super::tables::*;
use super::{SilkDecoder, SilkFrame};
use crate::error::Result;
use crate::range::RangeDecoder;

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

/// Deterministic pseudo-random bytes: SILK decodes any bytes as some frame.
fn noise(seed: u32, len: usize) -> Vec<u8> {
    let mut s = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..len)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s >> 24) as u8
        })
        .collect()
}

fn frame(frame_size: usize, rate: u32, stream_channels: usize) -> SilkFrame {
    SilkFrame {
        frame_size,
        internal_rate: rate,
        stream_channels,
        fec: false,
    }
}

/// Decodes `bytes` as one frame; returns the output.
fn decode(d: &mut SilkDecoder, f: &SilkFrame, bytes: &[u8]) -> Result<Vec<f32>> {
    let mut out = vec![0.0; f.frame_size * d.channels];
    let mut dec = RangeDecoder::new(bytes);
    d.decode(Some(&mut dec), f, &mut out)?;
    Ok(out)
}

fn conceal(d: &mut SilkDecoder, frame_size: usize) -> Result<Vec<f32>> {
    let mut out = vec![7.0; frame_size * d.channels];
    let r = d.decode(None, &frame(frame_size, 0, 0), &mut out);
    assert!(out.iter().all(|&s| s != 7.0), "every sample written");
    r.map(|()| out)
}

fn bounded(pcm: &[f32]) -> bool {
    pcm.iter().all(|s| s.is_finite() && s.abs() <= 1.0)
}

#[test]
fn concealment_before_any_frame_is_an_error_and_silence() {
    let mut d = SilkDecoder::new(2);
    let mut out = vec![1.0; 960 * 2];
    assert!(d.decode(None, &frame(960, 0, 0), &mut out).is_err());
    assert!(out.iter().all(|&s| s == 0.0));
}

#[test]
fn concealment_has_the_asked_length_is_bounded_and_recovers() {
    for (rate, ch, out_ch) in [(8000, 1, 1), (12000, 2, 2), (16000, 1, 2), (16000, 2, 1)] {
        let mut d = SilkDecoder::new(out_ch);
        for i in 0..6 {
            let f = frame(960, rate, ch);
            assert!(bounded(&decode(&mut d, &f, &noise(i, 60)).unwrap()));
        }
        // Whole frames, and less than 10 ms (the first samples of a 10 ms concealment).
        for size in [960, 480, 240, 120, 960, 1920, 2880] {
            let pcm = conceal(&mut d, size).unwrap();
            assert_eq!(pcm.len(), size * out_ch);
            assert!(bounded(&pcm), "{rate} {size}");
        }
        // A good frame after concealment decodes, its rate and channels whatever they are.
        for (i, (r, c)) in [(rate, ch), (8000, 2), (16000, 1)].into_iter().enumerate() {
            let pcm = decode(&mut d, &frame(480, r, c), &noise(100 + i as u32, 40)).unwrap();
            assert!(bounded(&pcm));
        }
    }
}

#[test]
fn concealment_fades_out() {
    let mut d = SilkDecoder::new(1);
    for i in 0..10 {
        decode(&mut d, &frame(960, 16000, 1), &noise(i, 80)).unwrap();
    }
    let energy = |p: &[f32]| p.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
    let first = energy(&conceal(&mut d, 960).unwrap());
    let mut last = first;
    for _ in 0..50 {
        last = energy(&conceal(&mut d, 960).unwrap());
    }
    assert!(last <= first, "{first} -> {last}");
}

#[test]
fn silence_stays_silence() {
    // After a reset the concealment has nothing to extend: it is silent, CNG included.
    let mut d = SilkDecoder::new(2);
    decode(&mut d, &frame(960, 12000, 2), &noise(1, 50)).unwrap();
    d.reset();
    for size in [960, 480, 120] {
        assert!(conceal(&mut d, size).unwrap().iter().all(|&s| s == 0.0));
    }
}

#[test]
fn concealment_is_deterministic() {
    let run = || {
        let mut d = SilkDecoder::new(2);
        let mut all = vec![];
        for i in 0..4 {
            all.extend(decode(&mut d, &frame(1920, 8000, 2), &noise(i, 120)).unwrap());
            all.extend(conceal(&mut d, 960).unwrap());
        }
        all
    };
    assert_eq!(run(), run());
}

#[test]
fn fec_decodes_a_frames_lbrr_data_without_panic() {
    for size in [480, 960, 1920, 2880] {
        for ch in [1, 2] {
            let mut d = SilkDecoder::new(2);
            for i in 0..8 {
                let f = SilkFrame {
                    fec: i % 2 == 1,
                    ..frame(size, 16000, ch)
                };
                let pcm = decode(&mut d, &f, &noise(i, 200)).unwrap();
                assert!(bounded(&pcm));
            }
        }
    }
}

#[test]
fn reset_forgets_everything_but_the_last_rate() {
    let f = frame(960, 16000, 2);
    let mut fresh = SilkDecoder::new(2);
    let want = decode(&mut fresh, &f, &noise(9, 70)).unwrap();
    let mut d = SilkDecoder::new(2);
    for i in 0..5 {
        decode(&mut d, &frame(480, 8000, 1), &noise(i, 30)).unwrap();
    }
    d.reset();
    assert_eq!(decode(&mut d, &f, &noise(9, 70)).unwrap(), want);
}

#[test]
fn bad_arguments_are_errors() {
    let mut d = SilkDecoder::new(1);
    let mut out = vec![0.0; 100];
    let mut dec = RangeDecoder::new(&[1, 2, 3]);
    assert!(
        d.decode(Some(&mut dec), &frame(960, 16000, 1), &mut out)
            .is_err()
    );
    let mut out = vec![0.0; 720];
    let mut dec = RangeDecoder::new(&[1, 2, 3]);
    assert!(
        d.decode(Some(&mut dec), &frame(720, 16000, 1), &mut out)
            .is_err()
    );
    let mut out = vec![0.0; 960];
    let mut dec = RangeDecoder::new(&[1, 2, 3]);
    assert!(
        d.decode(Some(&mut dec), &frame(960, 44100, 1), &mut out)
            .is_err()
    );
    assert!(
        d.decode(Some(&mut dec), &frame(960, 16000, 3), &mut out)
            .is_err()
    );
}

/// One step of a hostile script: a frame with data (any bytes, rate, channels, size, FEC) or
/// a loss of any length the top level may ask for.
#[derive(Debug, Clone)]
enum Step {
    Data(Vec<u8>, u32, usize, usize, bool),
    Lost(usize),
    Reset,
}

fn step() -> impl proptest::strategy::Strategy<Value = Step> {
    use proptest::prelude::*;
    prop_oneof![
        8 => (
            proptest::collection::vec(any::<u8>(), 0..300),
            prop_oneof![Just(8000u32), Just(12000), Just(16000)],
            1usize..=2,
            prop_oneof![Just(480usize), Just(960), Just(1920), Just(2880)],
            any::<bool>(),
        )
            .prop_map(|(b, r, c, s, f)| Step::Data(b, r, c, s, f)),
        3 => prop_oneof![Just(120usize), Just(240), Just(480), Just(960), Just(1920), Just(2880)]
            .prop_map(Step::Lost),
        1 => Just(Step::Reset),
    ]
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig { cases: 1500, ..Default::default() })]

    #[test]
    fn any_script_of_frames_losses_and_resets_is_safe(
        channels in 1usize..=2,
        script in proptest::collection::vec(step(), 1..16),
    ) {
        let mut d = SilkDecoder::new(channels);
        for s in script {
            match s {
                Step::Data(bytes, rate, ch, size, fec) => {
                    let f = SilkFrame { fec, ..frame(size, rate, ch) };
                    let pcm = decode(&mut d, &f, &bytes).unwrap();
                    proptest::prop_assert!(bounded(&pcm));
                }
                Step::Lost(size) => {
                    // Before any frame (or with nothing to conceal) it is an error and silence.
                    if let Ok(pcm) = conceal(&mut d, size) {
                        proptest::prop_assert_eq!(pcm.len(), size * channels);
                        proptest::prop_assert!(bounded(&pcm));
                    }
                }
                Step::Reset => d.reset(),
            }
        }
    }
}
