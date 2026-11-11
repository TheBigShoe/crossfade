//! The mix filter: standard down- and up-mixes by speaker position, or a given matrix.

use core::f64::consts::FRAC_1_SQRT_2 as R;
use crossfade_core::{
    AudioFrame, AudioSpec, Channel, ChannelLayout, Filter, SampleFormat, Samples,
};
use crossfade_dsp::filters::mix::{Mix, MixPlan};

fn layout(name: &str) -> ChannelLayout {
    ChannelLayout::from_name(name).unwrap_or_else(|| panic!("{name}"))
}

fn spec(l: ChannelLayout) -> AudioSpec {
    AudioSpec::new(48000, SampleFormat::F64, l).unwrap()
}

fn plan(to: &str) -> MixPlan {
    MixPlan {
        layout: layout(to),
        matrix: None,
    }
}

fn mix(from: &str, to: &str) -> Mix {
    Mix::new(&spec(layout(from)), &plan(to)).unwrap()
}

fn refusal(from: ChannelLayout, to: &str) -> String {
    match Mix::new(&spec(from), &plan(to)) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("expected a refusal"),
    }
}

fn run(f: &mut Mix, input: &AudioSpec, chunks: &[(i64, Vec<f64>)]) -> Vec<AudioFrame> {
    let mut got = Vec::new();
    let mut out = AudioFrame::new(f.output_spec());
    for (pts, c) in chunks {
        let frame = AudioFrame {
            pts: *pts,
            samples: Samples::F64(c.clone()),
            ..AudioFrame::new(input)
        };
        f.send_frame(&frame).unwrap();
        while f.receive_frame(&mut out).unwrap() {
            got.push(out.clone());
        }
    }
    f.drain();
    while f.receive_frame(&mut out).unwrap() {
        got.push(out.clone());
    }
    got
}

fn f64s(frames: &[AudioFrame]) -> Vec<f64> {
    frames
        .iter()
        .flat_map(|f| match &f.samples {
            Samples::F64(s) => s.clone(),
            _ => panic!(),
        })
        .collect()
}

/// One frame, input `i` holding 2^i: each output then shows which inputs reached it.
fn pow2(channels: u16) -> Vec<f64> {
    (0..channels).map(|i| f64::from(1u32 << i)).collect()
}

fn one_frame(from: &str, to: &str) -> Vec<f64> {
    let input = spec(layout(from));
    let mut m = mix(from, to);
    f64s(&run(&mut m, &input, &[(0, pow2(layout(from).channels()))]))
}

fn near(got: &[f64], want: &[f64]) {
    assert_eq!(got.len(), want.len());
    for (g, w) in got.iter().zip(want) {
        assert!(
            (g - w).abs() <= 8.0 * f64::EPSILON * w.abs().max(1.0),
            "{got:?} vs {want:?}"
        );
    }
}

#[test]
fn identical_layouts_copy_every_bit() {
    let input = spec(layout("5.1"));
    let odd = vec![-0.0, f64::MIN_POSITIVE, 1e300, 0.1, f64::INFINITY, -2.5];
    let mut m = mix("5.1", "5.1");
    let got = f64s(&run(&mut m, &input, &[(3, odd.clone())]));
    assert!(
        got.iter()
            .zip(&odd)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
}

#[test]
fn the_same_positions_in_another_order_are_a_permutation() {
    // 5.1 and 5.1-back differ in surrounds; 5.1 into a reordered copy of itself does not.
    let from =
        ChannelLayout::from_positions(vec![Channel::FrontRight, Channel::FrontLeft]).unwrap();
    let input = spec(from);
    let mut m = Mix::new(&input, &plan("stereo")).unwrap();
    let got = f64s(&run(&mut m, &input, &[(0, vec![-0.0, 8.0])]));
    assert_eq!(got[0], 8.0);
    assert_eq!(got[1].to_bits(), (-0.0f64).to_bits());
}

#[test]
fn mono_to_stereo_gives_each_side_the_whole_signal() {
    assert_eq!(one_frame("mono", "stereo"), [1.0, 1.0]);
}

#[test]
fn stereo_to_mono_averages() {
    assert_eq!(one_frame("stereo", "mono"), [1.5]);
}

#[test]
fn five_one_down_to_stereo_is_bs775_normalised() {
    // 5.1 order: FL FR FC LFE SL SR, so 1 2 4 8 16 32; LFE (8) never appears.
    let k = 1.0 + R + R;
    near(
        &one_frame("5.1", "stereo"),
        &[
            (1.0 + 4.0 * R + 16.0 * R) / k,
            (2.0 + 4.0 * R + 32.0 * R) / k,
        ],
    );
    // 5.1-back: BL BR are 16 32 and count as surrounds just the same.
    near(
        &one_frame("5.1-back", "stereo"),
        &[
            (1.0 + 4.0 * R + 16.0 * R) / k,
            (2.0 + 4.0 * R + 32.0 * R) / k,
        ],
    );
}

#[test]
fn five_oh_and_seven_one_and_six_one_down_to_stereo() {
    let k = 1.0 + R + R;
    // 5.0: FL FR FC SL SR
    near(
        &one_frame("5.0", "stereo"),
        &[
            (1.0 + 4.0 * R + 8.0 * R) / k,
            (2.0 + 4.0 * R + 16.0 * R) / k,
        ],
    );
    // 7.1: FL FR FC LFE BL BR SL SR; both surround pairs at 0.7071.
    let k7 = 1.0 + R + R + R;
    near(
        &one_frame("7.1", "stereo"),
        &[
            (1.0 + 4.0 * R + 16.0 * R + 64.0 * R) / k7,
            (2.0 + 4.0 * R + 32.0 * R + 128.0 * R) / k7,
        ],
    );
    // 6.1: FL FR FC LFE BC SL SR; the back centre feeds both sides at 0.7071.
    near(
        &one_frame("6.1", "stereo"),
        &[
            (1.0 + 4.0 * R + 16.0 * R + 32.0 * R) / k7,
            (2.0 + 4.0 * R + 16.0 * R + 64.0 * R) / k7,
        ],
    );
}

#[test]
fn down_to_mono_is_the_stereo_mix_then_the_average_in_one_matrix() {
    let k = 1.0 + R + R;
    let l = 1.0 + 4.0 * R + 16.0 * R;
    let r = 2.0 + 4.0 * R + 32.0 * R;
    near(&one_frame("5.1", "mono"), &[0.5 * (l + r) / k]);
}

#[test]
fn full_scale_on_every_input_gives_full_scale_out_and_never_more() {
    for from in [
        "stereo", "3.0", "quad", "5.0", "5.1", "5.1-back", "6.1", "7.1",
    ] {
        for to in ["stereo", "mono"] {
            if from == "stereo" && to == "stereo" {
                continue;
            }
            let n = layout(from).channels();
            let input = spec(layout(from));
            let mut m = mix(from, to);
            let got = f64s(&run(&mut m, &input, &[(0, vec![1.0; n as usize])]));
            for g in &got {
                assert_eq!(*g, 1.0, "{from} -> {to}");
            }
            let mut m = mix(from, to);
            let got = f64s(&run(&mut m, &input, &[(0, vec![-1.0; n as usize])]));
            assert!(got.iter().all(|g| *g == -1.0), "{from} -> {to}");
        }
    }
}

#[test]
fn lfe_is_dropped_in_every_downmix() {
    for from in ["2.1", "3.1", "5.1", "5.1-back", "6.1", "7.1"] {
        for to in ["stereo", "mono"] {
            let l = layout(from);
            let lfe = l
                .positions()
                .unwrap()
                .iter()
                .position(|c| *c == Channel::Lfe)
                .unwrap();
            let mut x = vec![0.0; l.channels() as usize];
            x[lfe] = 1.0;
            let input = spec(l);
            let mut m = mix(from, to);
            let got = f64s(&run(&mut m, &input, &[(0, x)]));
            assert!(got.iter().all(|g| *g == 0.0), "{from} -> {to}: {got:?}");
        }
    }
}

#[test]
fn upmixes_invent_nothing() {
    // 5.1 order FL FR FC LFE SL SR
    assert_eq!(one_frame("stereo", "5.1"), [1.0, 2.0, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(one_frame("mono", "5.1"), [0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
    assert_eq!(
        one_frame("stereo", "7.1"),
        [1.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
    );
}

#[test]
fn a_given_matrix_is_used_as_it_is() {
    let input = spec(layout("stereo"));
    let p = MixPlan {
        layout: layout("mono"),
        matrix: Some(vec![vec![2.0, -0.25]]),
    };
    let mut m = Mix::new(&input, &p).unwrap();
    let got = f64s(&run(
        &mut m,
        &input,
        &[(0, vec![1.0, 4.0]), (1, vec![0.5, 0.5])],
    ));
    assert_eq!(got, [1.0, 0.875]);
    // Rows are outputs, columns inputs; nothing is normalised (this one sums past 1).
    let p = MixPlan {
        layout: layout("stereo"),
        matrix: Some(vec![vec![0.0, 3.0], vec![3.0, 0.0]]),
    };
    let mut m = Mix::new(&input, &p).unwrap();
    assert_eq!(
        f64s(&run(&mut m, &input, &[(0, vec![1.0, 2.0])])),
        [6.0, 3.0]
    );
}

#[test]
fn a_matrix_of_the_wrong_shape_is_refused() {
    let input = spec(layout("stereo"));
    let p = MixPlan {
        layout: layout("mono"),
        matrix: Some(vec![vec![1.0]]),
    };
    assert!(Mix::new(&input, &p).is_err());
}

#[test]
fn unpositioned_channels_are_relabelled_when_the_count_fits() {
    let input = spec(ChannelLayout::unpositioned(4).unwrap());
    let mut m = Mix::new(&input, &plan("quad")).unwrap();
    assert_eq!(m.output_spec().layout(), &layout("quad"));
    let got = f64s(&run(&mut m, &input, &[(0, vec![1.0, 2.0, 3.0, 4.0])]));
    assert_eq!(got, [1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn pairs_without_a_standard_mix_are_refused_naming_both_and_matrix() {
    let e = refusal(ChannelLayout::unpositioned(4).unwrap(), "5.1");
    assert_eq!(
        e,
        "unsupported: no standard mix from 4 channels to 5.1; give one with mix ... matrix=..."
    );
    let e = refusal(layout("5.1"), "quad");
    assert!(e.contains("no standard mix from 5.1 to quad"), "{e}");
    assert!(e.contains("matrix="), "{e}");
}

#[test]
fn chunking_never_changes_a_bit() {
    let input = spec(layout("5.1"));
    let data: Vec<f64> = (0..6 * 23)
        .map(|i| (f64::from(i) * 0.137).fract() - 0.5)
        .collect();
    let whole = f64s(&run(
        &mut mix("5.1", "stereo"),
        &input,
        &[(0, data.clone())],
    ));
    for size in [1usize, 3, 7, 22] {
        let mut chunks = Vec::new();
        let mut at = 0;
        while at < 23 {
            let n = size.min(23 - at);
            chunks.push((at as i64, data[at * 6..(at + n) * 6].to_vec()));
            at += n;
        }
        let got = run(&mut mix("5.1", "stereo"), &input, &chunks);
        let pts: Vec<i64> = got.iter().map(|f| f.pts).collect();
        assert_eq!(pts, chunks.iter().map(|c| c.0).collect::<Vec<_>>());
        let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&f64s(&got)), bits(&whole), "chunk {size}");
    }
}

#[test]
fn an_empty_frame_gives_nothing_and_output_spec_is_the_new_layout() {
    let input = spec(layout("5.1"));
    let mut m = mix("5.1", "stereo");
    assert_eq!(m.output_spec(), &spec(layout("stereo")));
    assert!(run(&mut m, &input, &[(0, vec![])]).is_empty());
}

#[test]
fn a_frame_that_is_not_f64_is_refused() {
    let input = spec(layout("stereo"));
    let mut m = mix("stereo", "mono");
    let wrong = AudioFrame {
        samples: Samples::S16(vec![1, 2]),
        ..AudioFrame::new(&input)
    };
    assert!(m.send_frame(&wrong).is_err());
}

#[test]
fn unlabelled_channels_take_flacs_default_layout_for_a_standard_downmix() {
    use crossfade_dsp::filters::mix::assumption;
    let six = ChannelLayout::unpositioned(6).unwrap();
    let input = spec(six.clone());
    let mut m = Mix::new(&input, &plan("stereo")).unwrap();
    let mut labelled = mix("5.1", "stereo");
    let x: Vec<f64> = (0..6 * 7).map(|i| f64::from(i) / 64.0).collect();
    assert_eq!(
        f64s(&run(&mut m, &input, &[(0, x.clone())])),
        f64s(&run(&mut labelled, &spec(layout("5.1")), &[(0, x)]))
    );
    assert_eq!(
        assumption(&six, &layout("stereo")).as_deref(),
        Some("treating 6 unlabelled channels as 5.1")
    );
    let three = ChannelLayout::unpositioned(3).unwrap();
    assert!(Mix::new(&spec(three.clone()), &plan("mono")).is_ok());
    assert_eq!(
        assumption(&three, &layout("mono")).as_deref(),
        Some("treating 3 unlabelled channels as 3.0")
    );
    // Same count: a relabel, no assumption needed.
    assert_eq!(assumption(&six, &layout("5.1")), None);
    // No default for nine channels: refused, with the matrix hint.
    let nine = ChannelLayout::unpositioned(9).unwrap();
    assert!(refusal(nine.clone(), "stereo").contains("matrix="));
    assert_eq!(assumption(&nine, &layout("stereo")), None);
    // Mixing up to an unpositioned count stays refused.
    let up = MixPlan {
        layout: ChannelLayout::unpositioned(3).unwrap(),
        matrix: None,
    };
    assert!(Mix::new(&spec(layout("stereo")), &up).is_err());
    // A refusal still names the input as it is, not the assumed layout.
    assert_eq!(
        refusal(ChannelLayout::unpositioned(4).unwrap(), "5.1"),
        "unsupported: no standard mix from 4 channels to 5.1; give one with mix ... matrix=..."
    );
}
