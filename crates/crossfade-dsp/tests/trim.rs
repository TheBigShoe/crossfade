//! The trim filter: sample-accurate cuts, pts, chunking invariance.

use crossfade_core::{AudioFrame, AudioSpec, ChannelLayout, Filter, SampleFormat, Samples};
use crossfade_dsp::filters::trim::{Trim, TrimPlan};

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

/// Sends `data` (interleaved, `ch` channels) in chunks of `chunk` frames starting at `pts0`.
fn run(
    t: &mut Trim,
    input: &AudioSpec,
    ch: usize,
    data: &[f64],
    chunk: usize,
    pts0: i64,
) -> Vec<AudioFrame> {
    let mut got = Vec::new();
    let mut out = AudioFrame::new(t.output_spec());
    let mut pts = pts0;
    for c in data.chunks(chunk * ch) {
        t.send_frame(&frame(input, pts, c.to_vec())).unwrap();
        pts += (c.len() / ch) as i64;
        while t.receive_frame(&mut out).unwrap() {
            got.push(out.clone());
        }
    }
    t.drain();
    while t.receive_frame(&mut out).unwrap() {
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

fn ramp(n: usize) -> Vec<f64> {
    (0..n).map(|i| i as f64).collect()
}

fn check_contiguous(frames: &[AudioFrame], ch: usize, first: i64) {
    let mut want = first;
    for f in frames {
        assert!(!flat(std::slice::from_ref(f)).is_empty(), "empty frame");
        assert_eq!(f.pts, want);
        want += (flat(std::slice::from_ref(f)).len() / ch) as i64;
    }
}

#[test]
fn cuts_inside_frames_for_every_chunk_size() {
    let input = spec(ChannelLayout::mono());
    let x = ramp(40);
    for chunk in (1..=7).chain([40]) {
        let mut t = Trim::new(
            &input,
            &TrimPlan {
                start: 11,
                end: Some(29),
            },
        );
        let got = run(&mut t, &input, 1, &x, chunk, 0);
        assert_eq!(flat(&got), x[11..29], "chunk {chunk}");
        check_contiguous(&got, 1, 0);
    }
}

#[test]
fn stereo_cuts_whole_frames() {
    let input = spec(ChannelLayout::stereo());
    let x = ramp(60);
    for chunk in [1, 3, 5, 30] {
        let mut t = Trim::new(
            &input,
            &TrimPlan {
                start: 7,
                end: Some(20),
            },
        );
        let got = run(&mut t, &input, 2, &x, chunk, 0);
        assert_eq!(flat(&got), x[14..40], "chunk {chunk}");
    }
}

#[test]
fn start_zero_and_no_end_pass_everything() {
    let input = spec(ChannelLayout::mono());
    let x = ramp(25);
    for chunk in [1, 4, 25] {
        let mut t = Trim::new(
            &input,
            &TrimPlan {
                start: 0,
                end: None,
            },
        );
        assert_eq!(flat(&run(&mut t, &input, 1, &x, chunk, 0)), x);
    }
}

#[test]
fn end_past_the_input_is_the_input() {
    let input = spec(ChannelLayout::mono());
    let x = ramp(10);
    let mut t = Trim::new(
        &input,
        &TrimPlan {
            start: 3,
            end: Some(1000),
        },
    );
    assert_eq!(flat(&run(&mut t, &input, 1, &x, 4, 0)), x[3..]);
}

#[test]
fn start_past_the_end_of_input_gives_nothing() {
    let input = spec(ChannelLayout::mono());
    let mut t = Trim::new(
        &input,
        &TrimPlan {
            start: 50,
            end: None,
        },
    );
    assert!(run(&mut t, &input, 1, &ramp(10), 3, 0).is_empty());
}

#[test]
fn empty_range_gives_nothing() {
    let input = spec(ChannelLayout::mono());
    for (s, e) in [(5, 5), (8, 3)] {
        let mut t = Trim::new(
            &input,
            &TrimPlan {
                start: s,
                end: Some(e),
            },
        );
        assert!(run(&mut t, &input, 1, &ramp(20), 6, 0).is_empty());
    }
}

#[test]
fn first_output_carries_the_first_input_pts() {
    let input = spec(ChannelLayout::mono());
    let mut t = Trim::new(
        &input,
        &TrimPlan {
            start: 10,
            end: Some(15),
        },
    );
    let got = run(&mut t, &input, 1, &ramp(30), 4, 1000);
    assert_eq!(got[0].pts, 1000);
    check_contiguous(&got, 1, 1000);
    assert_eq!(
        got.iter()
            .map(|f| flat(std::slice::from_ref(f)).len())
            .sum::<usize>(),
        5
    );
}

#[test]
fn finished_once_end_is_passed() {
    let input = spec(ChannelLayout::mono());
    let mut t = Trim::new(
        &input,
        &TrimPlan {
            start: 2,
            end: Some(6),
        },
    );
    assert!(!t.finished());
    let mut out = AudioFrame::new(t.output_spec());
    t.send_frame(&frame(&input, 0, ramp(4))).unwrap();
    while t.receive_frame(&mut out).unwrap() {}
    assert!(!t.finished());
    t.send_frame(&frame(&input, 4, ramp(4))).unwrap();
    assert!(t.finished());
    let empty = Trim::new(
        &input,
        &TrimPlan {
            start: 3,
            end: Some(3),
        },
    );
    assert!(empty.finished());
    assert!(
        !Trim::new(
            &input,
            &TrimPlan {
                start: 0,
                end: None
            }
        )
        .finished()
    );
}

#[test]
fn refuses_non_f64() {
    let input = spec(ChannelLayout::mono());
    let mut t = Trim::new(
        &input,
        &TrimPlan {
            start: 0,
            end: None,
        },
    );
    let f = AudioFrame {
        samples: Samples::S16(vec![1, 2]),
        ..AudioFrame::new(&input)
    };
    assert!(t.send_frame(&f).is_err());
}
