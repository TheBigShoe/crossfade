//! Edge cases: weights, gates, silence, short input, other rates, bad input.
mod common;

use common::*;
use crossfade_loudness::{ChannelRole, Error, Meter, Options};

fn integrated(rate: u32, roles: &[ChannelRole], samples: &[f64]) -> Option<f64> {
    let mut m = meter(rate, roles);
    m.push(samples);
    m.integrated()
}

#[test]
fn the_same_signal_reads_the_same_at_every_common_rate() {
    for rate in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
        let got = integrated(rate, &STEREO, &tone_db(rate, 2, -23.0, 5_000));
        near(got, -23.0, 0.05, &format!("{rate} Hz"));
    }
}

#[test]
fn unusual_rates_read_close_too() {
    for rate in [8_000, 11_025, 22_050, 32_000, 384_000, 1_048_575] {
        let ms = if rate > 200_000 { 1_000 } else { 5_000 };
        let got = integrated(rate, &STEREO, &tone_db(rate, 2, -23.0, ms));
        near(got, -23.0, 0.15, &format!("{rate} Hz"));
    }
}

#[test]
fn a_surround_channel_weighs_1_41() {
    let front = integrated(FS, &[ChannelRole::Left], &tone_db(FS, 1, -23.0, 5_000)).unwrap();
    let rear = integrated(FS, &[ChannelRole::LeftSurround], &tone_db(FS, 1, -23.0, 5_000)).unwrap();
    assert!((rear - front - 10.0 * 1.41f64.log10()).abs() < 1e-6);
    let c = integrated(FS, &[ChannelRole::Centre], &tone_db(FS, 1, -23.0, 5_000)).unwrap();
    let u = integrated(FS, &[ChannelRole::Unweighted], &tone_db(FS, 1, -23.0, 5_000)).unwrap();
    assert_eq!(c.to_bits(), front.to_bits());
    assert_eq!(u.to_bits(), front.to_bits());
}

#[test]
fn mono_counts_once_with_no_dual_mono_bonus() {
    // One channel carries half the power of the same level on two: 3 LU lower.
    let mono = integrated(FS, &[ChannelRole::Unweighted], &tone_db(FS, 1, -23.0, 5_000)).unwrap();
    let stereo = integrated(FS, &STEREO, &tone_db(FS, 2, -23.0, 5_000)).unwrap();
    assert!((stereo - mono - 10.0 * 2f64.log10()).abs() < 1e-6);
}

#[test]
fn lfe_and_excluded_channels_add_no_loudness() {
    let roles = [ChannelRole::Left, ChannelRole::Lfe, ChannelRole::Excluded];
    let mut loud = Vec::new();
    for f in tone_db(FS, 1, -23.0, 5_000) {
        loud.extend([f, 0.9 * f, 0.9 * f]);
    }
    let with = integrated(FS, &roles, &loud).unwrap();
    let alone = integrated(FS, &[ChannelRole::Left], &tone_db(FS, 1, -23.0, 5_000)).unwrap();
    assert_eq!(with.to_bits(), alone.to_bits());
}

#[test]
fn standard_layouts() {
    use ChannelRole::*;
    assert_eq!(ChannelRole::standard_layout(2), Some(&[Left, Right][..]));
    assert_eq!(ChannelRole::standard_layout(6).unwrap()[3], Lfe);
    for n in 1..=6 {
        assert_eq!(ChannelRole::standard_layout(n).unwrap().len(), n);
    }
    assert_eq!(ChannelRole::standard_layout(7), None);
    assert_eq!(ChannelRole::standard_layout(0), None);
}

#[test]
fn digital_silence_has_no_loudness_and_no_nan() {
    let mut m = Meter::new(FS, &STEREO, Options::default()).unwrap();
    m.push(&silence(FS, 2, 10_000));
    let r = m.measurements();
    assert_eq!(r.integrated, None);
    assert_eq!(r.momentary, None);
    assert_eq!(r.momentary_max, None);
    assert_eq!(r.short_term, None);
    assert_eq!(r.short_term_max, None);
    assert_eq!(r.loudness_range, None);
    assert_eq!(r.sample_peak, 0.0);
    assert_eq!(r.true_peak, Some(0.0));
    assert_eq!(r.frames, 480_000);
}

#[test]
fn a_file_shorter_than_one_block_has_no_integrated_loudness() {
    let mut m = meter(FS, &STEREO);
    m.push(&tone_db(FS, 2, -23.0, 300));
    assert_eq!(m.integrated(), None);
    assert_eq!(m.momentary(), None);
    assert_eq!(m.short_term(), None);
    assert!(m.sample_peak() > 0.0);
    let mut m = meter(FS, &STEREO);
    m.push(&tone_db(FS, 2, -23.0, 399));
    assert_eq!(m.integrated(), None);
    m.push(&tone_db(FS, 2, -23.0, 1));
    near(m.integrated(), -23.0, 0.1, "exactly one block");
}

#[test]
fn the_absolute_gate_is_at_minus_70_lufs() {
    near(integrated(FS, &STEREO, &tone_db(FS, 2, -69.0, 3_000)), -69.0, 0.1, "-69");
    assert_eq!(integrated(FS, &STEREO, &tone_db(FS, 2, -71.0, 3_000)), None);
}

#[test]
fn the_relative_gate_is_10_lu_under_the_gated_mean() {
    // -31 dBFS is 11 LU below -20 but above the gate 10 LU under the mean (about -33):
    // it counts. -35 is under it: dropped, leaving the loud part alone.
    let counted = integrated(FS, &STEREO, &steps(FS, &[(10_000, -20.0), (10_000, -31.0)]));
    near(counted, 10.0 * ((0.01f64 + 10f64.powf(-3.1)) / 2.0).log10(), 0.15, "kept");
    let dropped = integrated(FS, &STEREO, &steps(FS, &[(10_000, -20.0), (10_000, -35.0)]));
    near(dropped, -20.0, 0.05, "dropped");
}

#[test]
fn momentary_and_short_term_readings_start_at_their_window_lengths() {
    let mut m = meter(FS, &STEREO);
    m.push(&tone_db(FS, 2, -23.0, 380));
    assert_eq!(m.momentary(), None);
    m.push(&tone_db(FS, 2, -23.0, 20));
    assert!(m.momentary().is_some());
    assert_eq!(m.short_term(), None);
    m.push(&tone_db(FS, 2, -23.0, 2_600));
    assert!(m.short_term().is_some());
}

#[test]
fn non_finite_and_huge_samples_are_replaced_and_counted() {
    let mut clean = meter(FS, &STEREO);
    let mut dirty = meter(FS, &STEREO);
    let good = tone_db(FS, 2, -23.0, 2_000);
    let mut bad = good.clone();
    for i in (0..bad.len()).step_by(997) {
        bad[i] = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY][i % 3];
    }
    let mut zeroed = good;
    for i in (0..zeroed.len()).step_by(997) {
        zeroed[i] = 0.0;
    }
    clean.push(&zeroed);
    dirty.push(&bad);
    assert_eq!(dirty.sanitised_samples(), (bad.len() as u64).div_ceil(997));
    assert_eq!(clean.sanitised_samples(), 0);
    assert_eq!(dirty.integrated().unwrap().to_bits(), clean.integrated().unwrap().to_bits());
    assert_eq!(dirty.sample_peak().to_bits(), clean.sample_peak().to_bits());

    let mut m = Meter::new(FS, &STEREO, Options::default()).unwrap();
    m.push(&vec![f64::MAX; 2 * 48_000]);
    m.push(&vec![f64::MIN; 2 * 48_000]);
    assert_eq!(m.sanitised_samples(), 4 * 48_000);
    let r = m.measurements();
    for v in [r.integrated, r.momentary_max, r.short_term_max, r.loudness_range, r.true_peak]
        .into_iter()
        .flatten()
    {
        assert!(v.is_finite(), "{v}");
    }
}

#[test]
fn bad_configuration_is_an_error() {
    let opts = Options::default();
    assert_eq!(Meter::new(FS, &[], opts).unwrap_err(), Error::NoChannels);
    assert_eq!(
        Meter::new(FS, &[ChannelRole::Left; 65], opts).unwrap_err(),
        Error::TooManyChannels { count: 65 }
    );
    assert_eq!(Meter::new(7_999, &STEREO, opts).unwrap_err(), Error::SampleRate { rate: 7_999 });
    assert!(Meter::new(8_000, &[ChannelRole::Left; 64], opts).is_ok());
    for e in [Error::NoChannels, Error::TooManyChannels { count: 70 }] {
        assert!(!e.to_string().is_empty());
    }
}

#[test]
fn reset_forgets_the_programme_but_keeps_the_setup() {
    let mut m = Meter::new(FS, &STEREO, Options::default()).unwrap();
    m.push(&tone_db(FS, 2, -10.0, 5_000));
    m.reset();
    assert_eq!(m.frames(), 0);
    assert_eq!(m.integrated(), None);
    assert_eq!(m.sample_peak(), 0.0);
    assert_eq!(m.channels(), 2);
    assert_eq!(m.sample_rate(), FS);
    m.push(&tone_db(FS, 2, -23.0, 5_000));
    let mut fresh = Meter::new(FS, &STEREO, Options::default()).unwrap();
    fresh.push(&tone_db(FS, 2, -23.0, 5_000));
    assert_eq!(m.measurements(), fresh.measurements());
    assert_eq!(m.clone().finish(), fresh.finish());
}

#[test]
fn a_pushed_half_frame_waits_for_its_other_half() {
    let signal = tone_db(FS, 2, -23.0, 1_000);
    let mut whole = meter(FS, &STEREO);
    whole.push(&signal);
    let mut split = meter(FS, &STEREO);
    split.push(&[]);
    split.push(&signal[..1]);
    assert_eq!(split.frames(), 0);
    split.push(&signal[1..]);
    assert_eq!(split.frames(), whole.frames());
    assert_eq!(split.measurements(), whole.measurements());
}
