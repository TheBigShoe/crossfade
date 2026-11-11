//! The fade filter: curves, look-ahead release, short streams, chunking invariance.

use crossfade_core::{AudioFrame, AudioSpec, ChannelLayout, Error, Filter, SampleFormat, Samples};
use crossfade_dsp::filters::fade::{Curve, Fade, FadePlan};

const CURVES: [Curve; 3] = [Curve::Linear, Curve::EqualPower, Curve::SCurve];

fn spec(layout: ChannelLayout) -> AudioSpec {
    AudioSpec::new(1000, SampleFormat::F64, layout).unwrap()
}

fn frame(spec: &AudioSpec, pts: i64, samples: Vec<f64>) -> AudioFrame {
    AudioFrame {
        pts,
        samples: Samples::F64(samples),
        ..AudioFrame::new(spec)
    }
}

fn run(f: &mut Fade, input: &AudioSpec, ch: usize, data: &[f64], chunk: usize) -> Vec<AudioFrame> {
    let mut got = Vec::new();
    let mut out = AudioFrame::new(f.output_spec());
    let mut pts = 500;
    for c in data.chunks(chunk * ch) {
        f.send_frame(&frame(input, pts, c.to_vec())).unwrap();
        pts += (c.len() / ch) as i64;
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

fn flat(frames: &[AudioFrame]) -> Vec<f64> {
    frames
        .iter()
        .flat_map(|f| match &f.samples {
            Samples::F64(s) => s.clone(),
            _ => panic!("not f64"),
        })
        .collect()
}

/// An independent evaluation of each curve at `t = k / n`.
fn gain(c: Curve, k: u64, n: u64) -> f64 {
    let t = k as f64 / n as f64;
    match c {
        Curve::Linear => t,
        Curve::EqualPower => libm::sin(t * std::f64::consts::FRAC_PI_2),
        Curve::SCurve => (1.0 - libm::cos(t * std::f64::consts::PI)) / 2.0,
    }
}

fn plan(fade_in: u64, fade_out: u64, curve: Curve) -> FadePlan {
    FadePlan {
        fade_in,
        fade_out,
        curve,
    }
}

fn ones(n: usize) -> Vec<f64> {
    vec![1.0; n]
}

#[test]
fn fade_in_gains_at_the_edges_of_every_curve() {
    let input = spec(ChannelLayout::mono());
    let n = 10;
    for c in CURVES {
        let mut f = Fade::new(&input, &plan(n, 0, c)).unwrap();
        let got = flat(&run(&mut f, &input, 1, &ones(30), 7));
        assert_eq!(got.len(), 30);
        assert_eq!(got[0], 0.0, "{c:?} first frame is silent");
        for k in [1, 2, n - 1] {
            assert_eq!(got[k as usize], gain(c, k, n), "{c:?} k={k}");
        }
        assert!(got[1..n as usize].windows(2).all(|w| w[0] < w[1]), "{c:?}");
        assert!(
            got[n as usize..].iter().all(|&g| g == 1.0),
            "{c:?} untouched after"
        );
    }
}

#[test]
fn fade_out_mirrors_with_the_last_frame_silent() {
    let input = spec(ChannelLayout::mono());
    let (len, n) = (30usize, 10u64);
    for c in CURVES {
        let mut f = Fade::new(&input, &plan(0, n, c)).unwrap();
        let got = flat(&run(&mut f, &input, 1, &ones(len), 4));
        assert_eq!(got.len(), len);
        assert_eq!(got[len - 1], 0.0, "{c:?} last frame is silent");
        assert!(got[..len - n as usize].iter().all(|&g| g == 1.0), "{c:?}");
        for k in [1u64, 2, n - 1] {
            assert_eq!(got[len - 1 - k as usize], gain(c, k, n), "{c:?} k={k}");
        }
    }
}

#[test]
fn stereo_applies_one_gain_to_both_channels() {
    let input = spec(ChannelLayout::stereo());
    let mut f = Fade::new(&input, &plan(4, 4, Curve::Linear)).unwrap();
    let got = flat(&run(&mut f, &input, 2, &ones(20), 3));
    for i in 0..10 {
        assert_eq!(got[2 * i], got[2 * i + 1]);
    }
    assert_eq!(&got[..2], &[0.0, 0.0]);
    assert_eq!(&got[2..4], &[0.25, 0.25]);
    assert_eq!(&got[18..], &[0.0, 0.0]);
}

#[test]
fn nothing_is_released_until_fade_out_frames_are_held_then_pts_are_contiguous() {
    let input = spec(ChannelLayout::mono());
    let mut f = Fade::new(&input, &plan(0, 5, Curve::Linear)).unwrap();
    let mut out = AudioFrame::new(f.output_spec());
    f.send_frame(&frame(&input, 500, ones(3))).unwrap();
    assert!(!f.receive_frame(&mut out).unwrap(), "only 3 of 5 held");
    f.send_frame(&frame(&input, 503, ones(4))).unwrap();
    assert!(f.receive_frame(&mut out).unwrap());
    assert_eq!((out.pts, flat(&[out.clone()]).len()), (500, 2));
    assert!(!f.receive_frame(&mut out).unwrap());
    f.send_frame(&frame(&input, 507, ones(1))).unwrap();
    assert!(f.receive_frame(&mut out).unwrap());
    assert_eq!((out.pts, flat(&[out.clone()]).len()), (502, 1));
    f.drain();
    let mut tail = Vec::new();
    while f.receive_frame(&mut out).unwrap() {
        tail.push(out.clone());
    }
    assert_eq!(tail[0].pts, 503);
    assert_eq!(flat(&tail), [0.8, 0.6, 0.4, 0.2, 0.0].map(|t| t * 1.0));
}

#[test]
fn drain_hands_back_the_held_frames_in_bounded_pieces() {
    let input = spec(ChannelLayout::mono());
    let n = 20_000;
    let mut f = Fade::new(&input, &plan(0, n, Curve::Linear)).unwrap();
    let got = run(&mut f, &input, 1, &ones(n as usize), 5000);
    assert!(got.len() > 1);
    assert_eq!(flat(&got).len(), n as usize);
    let mut want = 500;
    for g in &got {
        assert_eq!(g.pts, want);
        want += flat(std::slice::from_ref(g)).len() as i64;
    }
}

#[test]
fn a_stream_shorter_than_both_fades_multiplies_them() {
    let input = spec(ChannelLayout::mono());
    let (n_in, n_out, len) = (8u64, 6u64, 10u64);
    for c in CURVES {
        let mut f = Fade::new(&input, &plan(n_in, n_out, c)).unwrap();
        let got = flat(&run(&mut f, &input, 1, &ones(len as usize), 3));
        for j in 0..len {
            let gi = if j < n_in { gain(c, j, n_in) } else { 1.0 };
            let k = len - 1 - j;
            let go = if k < n_out { gain(c, k, n_out) } else { 1.0 };
            assert_eq!(got[j as usize], gi * go, "{c:?} j={j}");
        }
    }
}

#[test]
fn a_stream_shorter_than_fade_out_is_the_tail_of_it() {
    let input = spec(ChannelLayout::mono());
    let (n, len) = (10u64, 4u64);
    let mut f = Fade::new(&input, &plan(0, n, Curve::Linear)).unwrap();
    let got = flat(&run(&mut f, &input, 1, &ones(len as usize), 2));
    let want: Vec<f64> = (0..len)
        .map(|j| gain(Curve::Linear, len - 1 - j, n))
        .collect();
    assert_eq!(got, want);
    assert_ne!(
        got[0], 0.0,
        "not silent: the fade started before this stream did"
    );
    assert_eq!(*got.last().unwrap(), 0.0);
}

#[test]
fn an_empty_stream_gives_nothing() {
    let input = spec(ChannelLayout::mono());
    let mut f = Fade::new(&input, &plan(5, 5, Curve::Linear)).unwrap();
    assert!(run(&mut f, &input, 1, &[], 1).is_empty());
}

#[test]
fn chunking_does_not_change_a_bit() {
    let input = spec(ChannelLayout::stereo());
    let x: Vec<f64> = (0..2 * 97).map(|i| f64::from(i) * 0.013 - 1.0).collect();
    for c in CURVES {
        let p = plan(23, 31, c);
        let whole = flat(&run(&mut Fade::new(&input, &p).unwrap(), &input, 2, &x, 97));
        assert_eq!(whole.len(), x.len());
        for chunk in [1, 2, 3, 5, 7, 40] {
            let got = flat(&run(
                &mut Fade::new(&input, &p).unwrap(),
                &input,
                2,
                &x,
                chunk,
            ));
            let same = got
                .iter()
                .zip(&whole)
                .all(|(a, b)| a.to_bits() == b.to_bits());
            assert!(same && got.len() == whole.len(), "{c:?} chunk {chunk}");
        }
    }
}

#[test]
fn a_fade_out_longer_than_ten_minutes_is_refused() {
    let input = spec(ChannelLayout::stereo());
    assert!(Fade::new(&input, &plan(0, 600 * 1000, Curve::Linear)).is_ok());
    let err = Fade::new(&input, &plan(0, 600 * 1000 + 1, Curve::Linear)).err();
    assert!(matches!(err, Some(Error::Unsupported { .. })), "{err:?}");
}

#[test]
fn refuses_non_f64() {
    let input = spec(ChannelLayout::mono());
    let mut f = Fade::new(&input, &plan(1, 1, Curve::Linear)).unwrap();
    let fr = AudioFrame {
        samples: Samples::S16(vec![1, 2]),
        ..AudioFrame::new(&input)
    };
    assert!(f.send_frame(&fr).is_err());
}
