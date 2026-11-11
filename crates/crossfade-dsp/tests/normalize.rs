//! The pieces the two-pass normalise is built from: roles, the measuring wrapper, the gain.

use crossfade_core::{AudioFrame, AudioSpec, Channel, ChannelLayout, Error, SampleFormat, Samples};
use crossfade_dsp::filters::normalize::{Measure, NormalizePlan, roles_for};
use crossfade_loudness::{ChannelRole as R, Measurements, Options, normalisation_gain};

fn spec(rate: u32, layout: ChannelLayout) -> AudioSpec {
    AudioSpec::new(rate, SampleFormat::F64, layout).unwrap()
}

fn frame(spec: &AudioSpec, samples: Vec<f64>) -> AudioFrame {
    AudioFrame {
        sample_rate: spec.sample_rate(),
        layout: spec.layout().clone(),
        pts: 0,
        samples: Samples::F64(samples),
    }
}

/// `seconds` of a 1 kHz sine at `dbfs` peak on the listed channels, silence on the rest.
fn tone(rate: u32, channels: usize, on: &[usize], dbfs: f64, seconds: u32) -> Vec<f64> {
    let amp = 10f64.powf(dbfs / 20.0);
    let n = (rate * seconds) as usize;
    let mut v = vec![0.0; n * channels];
    for i in 0..n {
        let s = amp * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / f64::from(rate)).sin();
        for &c in on {
            v[i * channels + c] = s;
        }
    }
    v
}

fn measure(spec: &AudioSpec, samples: &[f64], chunk_frames: usize) -> Measurements {
    let ch = usize::from(spec.layout().channels());
    let mut m = Measure::new(spec, Options::default()).unwrap();
    for part in samples.chunks(chunk_frames * ch) {
        m.push(&frame(spec, part.to_vec())).unwrap();
    }
    m.finish()
}

fn lufs(m: &Measurements) -> f64 {
    m.integrated.expect("has loudness")
}

#[test]
fn roles_of_every_named_layout() {
    let expect: [(&str, &[R]); 14] = [
        ("mono", &[R::Unweighted]),
        ("stereo", &[R::Left, R::Right]),
        ("2.1", &[R::Left, R::Right, R::Lfe]),
        ("3.0", &[R::Left, R::Right, R::Centre]),
        ("3.1", &[R::Left, R::Right, R::Centre, R::Lfe]),
        ("4.0", &[R::Left, R::Right, R::Centre, R::Unweighted]),
        (
            "quad",
            &[R::Left, R::Right, R::LeftSurround, R::RightSurround],
        ),
        (
            "quad-side",
            &[R::Left, R::Right, R::LeftSurround, R::RightSurround],
        ),
        (
            "5.0",
            &[
                R::Left,
                R::Right,
                R::Centre,
                R::LeftSurround,
                R::RightSurround,
            ],
        ),
        (
            "5.0-back",
            &[
                R::Left,
                R::Right,
                R::Centre,
                R::LeftSurround,
                R::RightSurround,
            ],
        ),
        (
            "5.1",
            &[
                R::Left,
                R::Right,
                R::Centre,
                R::Lfe,
                R::LeftSurround,
                R::RightSurround,
            ],
        ),
        (
            "5.1-back",
            &[
                R::Left,
                R::Right,
                R::Centre,
                R::Lfe,
                R::LeftSurround,
                R::RightSurround,
            ],
        ),
        (
            "6.1",
            &[
                R::Left,
                R::Right,
                R::Centre,
                R::Lfe,
                R::Unweighted,
                R::LeftSurround,
                R::RightSurround,
            ],
        ),
        (
            "7.1",
            &[
                R::Left,
                R::Right,
                R::Centre,
                R::Lfe,
                R::LeftSurround,
                R::RightSurround,
                R::LeftSurround,
                R::RightSurround,
            ],
        ),
    ];
    for (name, roles) in expect {
        let layout = ChannelLayout::from_name(name).unwrap();
        assert_eq!(roles_for(&layout), roles, "{name}");
    }
}

#[test]
fn unpositioned_layouts_use_the_standard_order_up_to_six_then_all_unweighted() {
    for n in 1..=6u16 {
        let layout = ChannelLayout::unpositioned(n).unwrap();
        assert_eq!(
            roles_for(&layout),
            R::standard_layout(usize::from(n)).unwrap(),
            "{n}"
        );
    }
    let seven = ChannelLayout::unpositioned(7).unwrap();
    assert_eq!(roles_for(&seven), [R::Unweighted; 7]);
}

#[test]
fn other_positions_are_unweighted() {
    let layout = ChannelLayout::from_positions(vec![
        Channel::BackCenter,
        Channel::TopCenter,
        Channel::FrontLeftOfCenter,
    ])
    .unwrap();
    assert_eq!(roles_for(&layout), [R::Unweighted; 3]);
}

#[test]
fn stereo_1khz_at_minus_23_dbfs_reads_minus_23_lufs() {
    let s = spec(48_000, ChannelLayout::stereo());
    let m = measure(&s, &tone(48_000, 2, &[0, 1], -23.0, 20), 4800);
    assert!((lufs(&m) + 23.0).abs() < 0.1, "{}", lufs(&m));
    assert!(m.true_peak.is_some());
}

#[test]
fn mono_counts_once() {
    // One channel at -23 dBFS reads 3 LU below the same signal on both stereo channels.
    let mono = spec(48_000, ChannelLayout::mono());
    let m = measure(&mono, &tone(48_000, 1, &[0], -20.0, 10), 4800);
    let stereo = spec(48_000, ChannelLayout::stereo());
    let n = measure(&stereo, &tone(48_000, 2, &[0, 1], -20.0, 10), 4800);
    assert!((lufs(&n) - lufs(&m) - 3.01).abs() < 0.02);
}

#[test]
fn lfe_is_left_out_of_loudness_but_in_the_peaks() {
    let s = spec(48_000, ChannelLayout::from_name("5.1").unwrap());
    let m = measure(&s, &tone(48_000, 6, &[3], -6.0, 5), 4800);
    assert_eq!(m.integrated, None);
    assert!(m.sample_peak > 0.49, "{}", m.sample_peak);
    assert!(m.true_peak.unwrap() >= m.sample_peak);
}

#[test]
fn both_surround_pairs_of_a_7_1_weigh_1_41() {
    let s = spec(48_000, ChannelLayout::from_name("7.1").unwrap());
    let front = lufs(&measure(&s, &tone(48_000, 8, &[0], -20.0, 10), 4800));
    // 7.1 order: FL FR FC LFE BL BR SL SR.
    for ch in [4, 5, 6, 7] {
        let m = lufs(&measure(&s, &tone(48_000, 8, &[ch], -20.0, 10), 4800));
        assert!(
            (m - front - 1.49).abs() < 0.02,
            "channel {ch}: {}",
            m - front
        );
    }
}

#[test]
fn an_unpositioned_three_channel_file_is_left_right_centre() {
    let s = spec(48_000, ChannelLayout::unpositioned(3).unwrap());
    let m = measure(&s, &tone(48_000, 3, &[0, 1, 2], -23.0, 10), 4800);
    let st = spec(48_000, ChannelLayout::stereo());
    let n = measure(&st, &tone(48_000, 2, &[0, 1], -23.0, 10), 4800);
    // Three weight-1 channels are 1.76 dB louder than two.
    assert!((lufs(&m) - lufs(&n) - 1.76).abs() < 0.02);
}

#[test]
fn silence_and_a_programme_under_400_ms_have_no_loudness() {
    let s = spec(48_000, ChannelLayout::stereo());
    let m = measure(&s, &vec![0.0; 2 * 48_000 * 3], 4800);
    assert_eq!(m.integrated, None);
    assert_eq!(m.sample_peak, 0.0);
    let short = tone(48_000, 2, &[0, 1], -10.0, 1)[..2 * 9600].to_vec(); // 200 ms
    assert_eq!(measure(&s, &short, 4800).integrated, None);
}

#[test]
fn any_split_into_frames_gives_the_same_bits() {
    for layout in [
        ChannelLayout::stereo(),
        ChannelLayout::from_name("5.1").unwrap(),
    ] {
        let ch = usize::from(layout.channels());
        let s = spec(44_100, layout);
        let mut x = tone(44_100, ch, &(0..ch).collect::<Vec<_>>(), -17.0, 4);
        x[7 * ch] = 0.9; // a peak between the interpolation phases
        let whole = measure(&s, &x, x.len());
        for chunk in [1, 7, 4410, 1001] {
            assert_eq!(measure(&s, &x, chunk), whole, "chunk {chunk}");
        }
    }
}

#[test]
fn rates_the_meter_cannot_take_are_unsupported_and_named() {
    for rate in [4_000, 7_999, 2_000_000] {
        let err = Measure::new(&spec(rate, ChannelLayout::stereo()), Options::default())
            .err()
            .unwrap();
        let Error::Unsupported { what } = err else {
            panic!("{err:?}")
        };
        assert!(what.contains(&rate.to_string()), "{what}");
    }
    for rate in [8_000, 192_000] {
        assert!(Measure::new(&spec(rate, ChannelLayout::stereo()), Options::default()).is_ok());
    }
}

#[test]
fn only_f64_frames_are_measured() {
    let s = spec(48_000, ChannelLayout::stereo());
    let mut m = Measure::new(&s, Options::default()).unwrap();
    let f = AudioFrame {
        sample_rate: 48_000,
        layout: ChannelLayout::stereo(),
        pts: 0,
        samples: Samples::S16(vec![0; 8]),
    };
    let err = m.push(&f).unwrap_err();
    assert!(matches!(err, Error::Unsupported { .. }), "{err:?}");
}

fn m_with(integrated: Option<f64>, sample_peak: f64, true_peak: Option<f64>) -> Measurements {
    let mut meter = crossfade_loudness::Meter::new(48_000, &[R::Left], Options::default()).unwrap();
    meter.push(&[]);
    Measurements {
        integrated,
        sample_peak,
        true_peak,
        ..meter.finish()
    }
}

#[test]
fn a_quiet_programme_is_raised_to_the_target() {
    let plan = NormalizePlan {
        target_lufs: -16.0,
        ceiling_dbtp: -1.0,
    };
    let g = plan.gain(&m_with(Some(-26.0), 0.1, Some(0.1)));
    assert!(!g.limited_by_ceiling);
    let want = normalisation_gain(Some(-26.0), -16.0, 0.1, -1.0);
    assert_eq!(g.gain.linear.to_bits(), want.linear.to_bits());
    assert!((g.gain.linear - 3.1622776601683795).abs() < 1e-12);
}

#[test]
fn a_loud_programme_is_lowered() {
    let plan = NormalizePlan {
        target_lufs: -23.0,
        ceiling_dbtp: -1.0,
    };
    let g = plan.gain(&m_with(Some(-10.0), 0.9, Some(0.9)));
    assert!(!g.limited_by_ceiling);
    assert!(g.gain.linear < 0.3);
}

#[test]
fn the_ceiling_limits_the_gain_and_says_so() {
    let plan = NormalizePlan {
        target_lufs: -10.0,
        ceiling_dbtp: -1.0,
    };
    let g = plan.gain(&m_with(Some(-26.0), 0.5, Some(0.5)));
    assert!(g.limited_by_ceiling);
    let want = normalisation_gain(Some(-26.0), -10.0, 0.5, -1.0);
    assert_eq!(g.gain.linear.to_bits(), want.linear.to_bits());
    // The true peak after the gain sits at the ceiling.
    assert!((g.gain.linear * 0.5 - 10f64.powf(-1.0 / 20.0)).abs() < 1e-12);
}

#[test]
fn the_sample_peak_stands_in_when_there_is_no_true_peak() {
    let plan = NormalizePlan {
        target_lufs: -10.0,
        ceiling_dbtp: -1.0,
    };
    let g = plan.gain(&m_with(Some(-26.0), 0.5, None));
    assert_eq!(
        g.gain.linear.to_bits(),
        normalisation_gain(Some(-26.0), -10.0, 0.5, -1.0)
            .linear
            .to_bits()
    );
}

#[test]
fn silence_gets_unity() {
    let plan = NormalizePlan {
        target_lufs: -16.0,
        ceiling_dbtp: -1.0,
    };
    let g = plan.gain(&m_with(None, 0.0, Some(0.0)));
    assert_eq!(g.gain.linear, 1.0);
    assert!(!g.limited_by_ceiling);
}
