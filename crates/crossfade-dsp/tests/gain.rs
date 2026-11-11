//! The gain filter and the dB rule every level-setting filter shares.

use crossfade_core::{AudioFrame, AudioSpec, ChannelLayout, Filter, SampleFormat, Samples};
use crossfade_dsp::filters::gain::{Gain, GainPlan};
use crossfade_dsp::level::db_to_linear;

fn spec(layout: ChannelLayout) -> AudioSpec {
    AudioSpec::new(44100, SampleFormat::F64, layout).unwrap()
}

fn frame(spec: &AudioSpec, pts: i64, samples: Vec<f64>) -> AudioFrame {
    AudioFrame {
        pts,
        samples: Samples::F64(samples),
        ..AudioFrame::new(spec)
    }
}

/// Sends each chunk and collects every frame out, draining at the end.
fn run(f: &mut dyn Filter, input: &AudioSpec, chunks: &[(i64, Vec<f64>)]) -> Vec<AudioFrame> {
    let mut got = Vec::new();
    let mut out = AudioFrame::new(f.output_spec());
    for (pts, c) in chunks {
        f.send_frame(&frame(input, *pts, c.clone())).unwrap();
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

#[test]
fn db_to_linear_is_exact_where_it_can_be() {
    assert_eq!(db_to_linear(0.0), 1.0);
    assert_eq!(db_to_linear(20.0), 10.0);
    assert_eq!(db_to_linear(-20.0), 0.1);
    assert_eq!(db_to_linear(-40.0), 0.01);
    // 20 log10(2) dB doubles, to the last bit or one off.
    let six = db_to_linear(6.020_599_913_279_624);
    assert!((six - 2.0).abs() <= 2.0 * f64::EPSILON, "{six}");
}

#[test]
fn db_to_linear_gives_pinned_bits() {
    // libm, not the platform's pow: the same bits on every machine. These are the correctly
    // rounded values of 10^(-6/20) and 10^(-3/20).
    assert_eq!(db_to_linear(-6.0).to_bits(), 0x3FE0_09B9_CF33_4252);
    assert_eq!(db_to_linear(-3.0).to_bits(), 0x3FE6_A77D_EAE8_AB8A);
}

#[test]
fn gain_multiplies_every_sample_and_keeps_pts_and_spec() {
    let input = spec(ChannelLayout::stereo());
    let mut g = Gain::new(&input, &GainPlan { linear: 0.5 });
    assert_eq!(g.output_spec(), &input);
    let got = run(
        &mut g,
        &input,
        &[(0, vec![1.0, -0.5, 0.25, 0.0]), (2, vec![-1.0, 3.0])],
    );
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].samples, Samples::F64(vec![0.5, -0.25, 0.125, 0.0]));
    assert_eq!(got[1].samples, Samples::F64(vec![-0.5, 1.5]));
    assert_eq!((got[0].pts, got[1].pts), (0, 2));
    assert!(got.iter().all(|f| f.spec() == input));
}

#[test]
fn unity_gain_changes_no_bit_and_an_empty_frame_gives_nothing() {
    let input = spec(ChannelLayout::mono());
    let mut g = Gain::new(&input, &GainPlan { linear: 1.0 });
    let odd = vec![f64::MIN_POSITIVE, -0.0, 1e300, 0.123_456_789];
    let got = run(&mut g, &input, &[(7, odd.clone()), (11, vec![])]);
    assert_eq!(got.len(), 1);
    let Samples::F64(s) = &got[0].samples else {
        panic!()
    };
    assert!(s.iter().zip(&odd).all(|(a, b)| a.to_bits() == b.to_bits()));
    assert_eq!(got[0].pts, 7);
}

#[test]
fn gain_refuses_a_frame_that_is_not_the_working_format() {
    let input = spec(ChannelLayout::mono());
    let mut g = Gain::new(&input, &GainPlan { linear: 2.0 });
    let wrong = AudioFrame {
        samples: Samples::S16(vec![1, 2]),
        ..AudioFrame::new(&input)
    };
    assert!(g.send_frame(&wrong).is_err());
}
