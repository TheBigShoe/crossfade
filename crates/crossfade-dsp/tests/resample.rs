//! The resample filter: `crossfade-resample` behind the `Filter` contract. Its output is the
//! crate's, bit for bit, whatever the framing; pts follow the contract; lengths follow the
//! crate's own rule.

use crossfade_core::{AudioFrame, AudioSpec, ChannelLayout, Filter, SampleFormat, Samples};
use crossfade_dsp::chain::{Chain, ChainPlan};
use crossfade_dsp::dither::Dither;
use crossfade_dsp::filters::resample::{Quality, Resample, ResamplePlan};
use crossfade_dsp::filters::{FilterPlan, build};
use crossfade_resample::{Quality as Preset, Resampler, output_len};

fn spec(rate: u32, layout: ChannelLayout) -> AudioSpec {
    AudioSpec::new(rate, SampleFormat::F64, layout).unwrap()
}

fn plan(rate: u32, quality: Quality) -> ResamplePlan {
    ResamplePlan { rate, quality }
}

/// A two-tone signal, interleaved, `frames` long.
fn signal(frames: usize, channels: usize) -> Vec<f64> {
    (0..frames * channels)
        .map(|i| {
            let t = (i / channels) as f64;
            let c = (i % channels) as f64;
            0.5 * libm::sin(t * 0.031 + c) + 0.25 * libm::sin(t * 0.47)
        })
        .collect()
}

/// Feeds `x` in chunks of `chunk` frames starting at `pts`, drains, returns (pts, samples) of
/// every frame out.
fn run(
    f: &mut dyn Filter,
    input: &AudioSpec,
    x: &[f64],
    chunk: usize,
    pts: i64,
) -> Vec<(i64, Vec<f64>)> {
    let ch = usize::from(input.layout().channels());
    let mut out = AudioFrame::new(f.output_spec());
    let mut got = Vec::new();
    let mut at = pts;
    for part in x.chunks(chunk * ch) {
        let frame = AudioFrame {
            pts: at,
            samples: Samples::F64(part.to_vec()),
            ..AudioFrame::new(input)
        };
        at += (part.len() / ch) as i64;
        f.send_frame(&frame).unwrap();
        while f.receive_frame(&mut out).unwrap() {
            assert!(!out.samples.is_empty(), "never an empty frame");
            assert_eq!(out.sample_rate, f.output_spec().sample_rate());
            let Samples::F64(s) = &out.samples else {
                panic!()
            };
            got.push((out.pts, s.clone()));
        }
    }
    f.drain();
    while f.receive_frame(&mut out).unwrap() {
        let Samples::F64(s) = &out.samples else {
            panic!()
        };
        got.push((out.pts, s.clone()));
    }
    got
}

fn reference(from: u32, to: u32, channels: usize, preset: Preset, x: &[f64]) -> Vec<f64> {
    let mut r = Resampler::new(from, to, channels, preset).unwrap();
    let mut out = Vec::new();
    r.process(x, &mut out).unwrap();
    r.flush(&mut out);
    out
}

#[test]
fn the_output_is_the_crates_bit_for_bit_with_contiguous_pts_for_any_framing() {
    let input = spec(44_100, ChannelLayout::stereo());
    let x = signal(10_000, 2);
    let want = reference(44_100, 48_000, 2, Preset::Balanced, &x);
    assert_eq!(want.len() as u64, 2 * output_len(10_000, 44_100, 48_000));
    for chunk in [1, 7, 256, 4096, 10_000] {
        let mut f = Resample::new(&input, &plan(48_000, Quality::Good)).unwrap();
        assert_eq!(f.output_spec(), &spec(48_000, ChannelLayout::stereo()));
        let got = run(&mut f, &input, &x, chunk, 0);
        let mut pts = 0;
        for (p, s) in &got {
            assert_eq!(*p, pts, "chunk {chunk}");
            pts += (s.len() / 2) as i64;
        }
        let all: Vec<f64> = got.into_iter().flat_map(|(_, s)| s).collect();
        assert!(
            all.iter()
                .zip(&want)
                .all(|(a, b)| a.to_bits() == b.to_bits())
                && all.len() == want.len(),
            "chunk {chunk}"
        );
    }
}

#[test]
fn qualities_map_to_the_crates_presets() {
    let input = spec(48_000, ChannelLayout::mono());
    let x = signal(3000, 1);
    for (q, preset) in [
        (Quality::Fast, Preset::Fast),
        (Quality::Good, Preset::Balanced),
        (Quality::Best, Preset::Best),
    ] {
        let mut f = Resample::new(&input, &plan(44_100, q)).unwrap();
        let all: Vec<f64> = run(&mut f, &input, &x, 500, 0)
            .into_iter()
            .flat_map(|(_, s)| s)
            .collect();
        assert_eq!(all, reference(48_000, 44_100, 1, preset, &x), "{q:?}");
    }
}

#[test]
fn the_first_pts_is_the_input_pts_at_the_new_rate() {
    let input = spec(44_100, ChannelLayout::mono());
    let mut f = Resample::new(&input, &plan(48_000, Quality::Fast)).unwrap();
    let got = run(&mut f, &input, &signal(1000, 1), 300, 44_100);
    assert_eq!(got[0].0, 48_000);
    // Rounded down when it does not land on a frame.
    let mut f = Resample::new(&input, &plan(48_000, Quality::Fast)).unwrap();
    assert_eq!(run(&mut f, &input, &signal(1000, 1), 300, 1)[0].0, 1);
    let mut f = Resample::new(&input, &plan(8_000, Quality::Fast)).unwrap();
    assert_eq!(run(&mut f, &input, &signal(1000, 1), 300, 10)[0].0, 1);
}

#[test]
fn equal_rates_pass_through_and_nothing_in_gives_nothing_out() {
    let input = spec(48_000, ChannelLayout::stereo());
    let x = signal(777, 2);
    let mut f = Resample::new(&input, &plan(48_000, Quality::Best)).unwrap();
    let all: Vec<f64> = run(&mut f, &input, &x, 100, 0)
        .into_iter()
        .flat_map(|(_, s)| s)
        .collect();
    assert_eq!(all, x);
    let mut f = Resample::new(&input, &plan(96_000, Quality::Best)).unwrap();
    assert!(run(&mut f, &input, &[], 100, 0).is_empty());
}

#[test]
fn a_frame_that_is_not_the_working_format_is_refused() {
    let input = spec(48_000, ChannelLayout::mono());
    let mut f = Resample::new(&input, &plan(44_100, Quality::Good)).unwrap();
    let wrong = AudioFrame {
        samples: Samples::S16(vec![1, 2]),
        ..AudioFrame::new(&input)
    };
    assert!(f.send_frame(&wrong).is_err());
}

#[test]
fn build_gives_a_running_resampler_and_the_chain_converts_integers() {
    let input = spec(44_100, ChannelLayout::stereo());
    let p = FilterPlan::Resample(plan(48_000, Quality::Good));
    assert_eq!(
        build(&p, &input).unwrap().output_spec().sample_rate(),
        48_000
    );
    let s16 = AudioSpec::new(44_100, SampleFormat::S16, ChannelLayout::stereo()).unwrap();
    let mut chain = Chain::new(
        &s16,
        &ChainPlan {
            filters: vec![p],
            dither: Dither::None,
        },
        SampleFormat::S24,
    )
    .unwrap();
    assert_eq!(chain.output_spec().sample_rate(), 48_000);
    let frame = AudioFrame {
        samples: Samples::S16(vec![1000; 2 * 4410]),
        ..AudioFrame::new(&s16)
    };
    chain.send_frame(&frame).unwrap();
    chain.drain();
    let mut out = AudioFrame::new(chain.output_spec());
    let mut n = 0;
    while chain.receive_frame(&mut out).unwrap() {
        n += out.frames();
    }
    assert_eq!(n, 4800);
}

#[test]
fn plan_lengths_follow_the_crates_rule() {
    for (n, from, to) in [
        (1, 44_100, 48_000),
        (10_007, 48_000, 44_100),
        (5, 8_000, 192_000),
        (99, 192_000, 8_000),
    ] {
        assert_eq!(
            plan(to, Quality::Good).output_frames(from, Some(n)),
            Some(output_len(n, from, to))
        );
    }
}
