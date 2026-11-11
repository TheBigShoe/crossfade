//! The chain: any input format in, the filters in order on `f64`, the output format out
//! through the quantiser; and the one place a planned filter becomes a running one.

use crossfade_core::{AudioFrame, AudioSpec, ChannelLayout, Error, Filter, SampleFormat, Samples};
use crossfade_dsp::chain::{Chain, ChainPlan};
use crossfade_dsp::dither::Dither;
use crossfade_dsp::filters::fade::{Curve, FadePlan};
use crossfade_dsp::filters::gain::GainPlan;
use crossfade_dsp::filters::mix::MixPlan;
use crossfade_dsp::filters::normalize::NormalizePlan;
use crossfade_dsp::filters::resample::{Quality, ResamplePlan};
use crossfade_dsp::filters::trim::TrimPlan;
use crossfade_dsp::filters::{FilterPlan, build};

fn spec(rate: u32, format: SampleFormat, layout: ChannelLayout) -> AudioSpec {
    AudioSpec::new(rate, format, layout).unwrap()
}

fn gain(linear: f64) -> FilterPlan {
    FilterPlan::Gain(GainPlan { linear })
}

fn plan(filters: Vec<FilterPlan>, dither: Dither) -> ChainPlan {
    ChainPlan { filters, dither }
}

/// Runs `frames` through `chain` in chunks of `chunk` frames and returns every frame out.
fn run(chain: &mut Chain, input: &AudioSpec, samples: Samples, chunk: usize) -> Vec<AudioFrame> {
    let ch = usize::from(input.layout().channels());
    let mut got = Vec::new();
    let mut out = AudioFrame::new(chain.output_spec());
    let n = samples.len() / ch;
    let mut start = 0;
    while start < n {
        let end = (start + chunk).min(n);
        let part = match &samples {
            Samples::S16(v) => Samples::S16(v[start * ch..end * ch].to_vec()),
            Samples::F32(v) => Samples::F32(v[start * ch..end * ch].to_vec()),
            Samples::F64(v) => Samples::F64(v[start * ch..end * ch].to_vec()),
            other => panic!("{other:?}"),
        };
        let frame = AudioFrame {
            pts: start as i64,
            samples: part,
            ..AudioFrame::new(input)
        };
        chain.send_frame(&frame).unwrap();
        while chain.receive_frame(&mut out).unwrap() {
            got.push(out.clone());
        }
        start = end;
    }
    chain.drain();
    while chain.receive_frame(&mut out).unwrap() {
        got.push(out.clone());
    }
    got
}

fn s16s(frames: &[AudioFrame]) -> Vec<i16> {
    frames
        .iter()
        .flat_map(|f| match &f.samples {
            Samples::S16(v) => v.clone(),
            other => panic!("{other:?}"),
        })
        .collect()
}

#[test]
fn a_gain_chain_on_s16_rounds_as_the_conversion_rule_does_without_dither() {
    let input = spec(8000, SampleFormat::S16, ChannelLayout::stereo());
    let mut chain = Chain::new(
        &input,
        &plan(vec![gain(0.5)], Dither::None),
        SampleFormat::S16,
    )
    .unwrap();
    assert_eq!(chain.output_spec(), &input);
    let x = vec![1000, -1000, 3, -3, 32767, -32768, 1, 0];
    let got = run(&mut chain, &input, Samples::S16(x), 3);
    // 3/2 and -3/2 round half away from zero; 32767/2 = 16383.5 rounds up.
    assert_eq!(s16s(&got), [500, -500, 2, -2, 16384, -16384, 1, 0]);
    assert_eq!(got.iter().map(|f| f.pts).collect::<Vec<_>>(), [0, 3]);
    assert_eq!(chain.clipped(), 0);
}

#[test]
fn filters_run_in_order_and_the_output_takes_the_requested_format() {
    let input = spec(48000, SampleFormat::F32, ChannelLayout::mono());
    let filters = vec![gain(4.0), gain(0.125)];
    let mut chain = Chain::new(&input, &plan(filters, Dither::None), SampleFormat::F64).unwrap();
    assert_eq!(chain.output_spec().format(), SampleFormat::F64);
    let got = run(&mut chain, &input, Samples::F32(vec![0.5, -0.25, 1.0]), 2);
    let all: Vec<f64> = got
        .iter()
        .flat_map(|f| match &f.samples {
            Samples::F64(v) => v.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(all, [0.25, -0.125, 0.5]);
}

#[test]
fn float_output_is_never_dithered() {
    let input = spec(44100, SampleFormat::F32, ChannelLayout::stereo());
    let x: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.013).sin() * 0.9).collect();
    let mut chain = Chain::new(
        &input,
        &plan(vec![gain(1.0)], Dither::Tpdf),
        SampleFormat::F32,
    )
    .unwrap();
    let got = run(&mut chain, &input, Samples::F32(x.clone()), 100);
    let all: Vec<f32> = got
        .iter()
        .flat_map(|f| match &f.samples {
            Samples::F32(v) => v.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(all, x);
}

#[test]
fn twenty_db_up_clips_and_is_counted() {
    let input = spec(8000, SampleFormat::S16, ChannelLayout::mono());
    let mut chain = Chain::new(
        &input,
        &plan(vec![gain(10.0)], Dither::Tpdf),
        SampleFormat::S16,
    )
    .unwrap();
    // 3276 * 10 fits, 3277 * 10 does not; so do the negatives down to -3276.8.
    let x = vec![3276, 3277, -3276, -3277, 0, 10_000, -10_000];
    let got = run(&mut chain, &input, Samples::S16(x), 7);
    assert_eq!(chain.clipped(), 4);
    let v = s16s(&got);
    assert_eq!((v[1], v[3], v[5], v[6]), (32767, -32768, 32767, -32768));
}

#[test]
fn the_bytes_do_not_depend_on_how_the_input_is_split() {
    let input = spec(48000, SampleFormat::S16, ChannelLayout::stereo());
    let x: Vec<i16> = (0..20_000)
        .map(|i| ((i * 7919) % 65536 - 32768) as i16)
        .collect();
    let once = |chunk| {
        let mut chain = Chain::new(
            &input,
            &plan(vec![gain(0.3)], Dither::Tpdf),
            SampleFormat::S16,
        )
        .unwrap();
        s16s(&run(&mut chain, &input, Samples::S16(x.clone()), chunk))
    };
    let whole = once(10_000);
    for chunk in [1, 333, 4096] {
        assert_eq!(once(chunk), whole, "{chunk}");
    }
}

#[test]
fn a_normalize_stage_builds_only_once_its_measured_gain_replaces_it() {
    let input = spec(48000, SampleFormat::F64, ChannelLayout::stereo());
    let normalize = FilterPlan::Normalize(NormalizePlan {
        target_lufs: -16.0,
        ceiling_dbtp: -1.0,
    });
    assert_eq!(normalize.name(), "normalize");
    let err = build(&normalize, &input).err().unwrap();
    assert!(err.to_string().contains("measuring pass"), "{err}");
    let chain = plan(vec![gain(0.5), normalize, gain(2.0)], Dither::Tpdf);
    assert!(Chain::new(&input, &chain, SampleFormat::S16).is_err());
    assert_eq!(chain.normalize().map(|(i, _)| i), Some(1));
    assert_eq!(chain.before_normalize().unwrap().filters, [gain(0.5)]);
    let measured = chain.with_measured_gain(GainPlan { linear: 4.0 });
    assert_eq!(measured.filters, [gain(0.5), gain(4.0), gain(2.0)]);
    assert!(Chain::new(&input, &measured, SampleFormat::S16).is_ok());
    assert_eq!(plan(vec![gain(1.0)], Dither::Tpdf).before_normalize(), None);
}

#[test]
fn the_plan_knows_the_output_before_anything_runs() {
    let input = spec(44100, SampleFormat::F64, ChannelLayout::stereo());
    let filters = vec![
        FilterPlan::Trim(TrimPlan {
            start: 44100,
            end: Some(441_000),
        }),
        FilterPlan::Resample(ResamplePlan {
            rate: 48000,
            quality: Quality::Best,
        }),
        FilterPlan::Mix(MixPlan {
            layout: ChannelLayout::from_name("5.1").unwrap(),
            matrix: None,
        }),
    ];
    let p = plan(filters, Dither::Tpdf);
    let (out, frames) = p.output(&input, Some(44100 * 60)).unwrap();
    assert_eq!(out.sample_rate(), 48000);
    assert_eq!(out.layout().name(), Some("5.1"));
    assert_eq!(out.format(), SampleFormat::F64);
    // 396,900 frames at 44.1 kHz are exactly 432,000 at 48 kHz.
    assert_eq!(frames, Some(432_000));
    // A trim with an end on an input of unknown length gives an unknown length.
    assert_eq!(p.output(&input, None).unwrap().1, None);
    // Shorter than the trim's start: nothing at all.
    assert_eq!(p.output(&input, Some(1000)).unwrap().1, Some(0));
}

#[test]
fn trim_resample_and_fade_frame_counts() {
    let input = spec(44100, SampleFormat::F64, ChannelLayout::mono());
    let frames = |p: FilterPlan, n| p.output_frames(&input, n);
    let open = FilterPlan::Trim(TrimPlan {
        start: 10,
        end: None,
    });
    assert_eq!(frames(open.clone(), Some(100)), Some(90));
    assert_eq!(frames(open, None), None);
    let closed = FilterPlan::Trim(TrimPlan {
        start: 10,
        end: Some(50),
    });
    assert_eq!(frames(closed.clone(), Some(100)), Some(40));
    assert_eq!(frames(closed, Some(30)), Some(20));
    // ceil(N * out / in): one input frame still gives one output frame.
    let up = FilterPlan::Resample(ResamplePlan {
        rate: 48000,
        quality: Quality::Fast,
    });
    assert_eq!(frames(up.clone(), Some(1)), Some(2));
    assert_eq!(frames(up.clone(), Some(44100)), Some(48000));
    assert_eq!(frames(up, Some(u64::MAX / 2)), None, "overflow is unknown");
    let fade = FilterPlan::Fade(FadePlan {
        fade_in: 5,
        fade_out: 5,
        curve: Curve::SCurve,
    });
    assert_eq!(frames(fade, Some(3)), Some(3));
}

#[test]
fn a_matrix_must_fit_the_input_and_the_layout() {
    let stereo = spec(48000, SampleFormat::F64, ChannelLayout::stereo());
    let mix = |rows: Vec<Vec<f64>>| {
        FilterPlan::Mix(MixPlan {
            layout: ChannelLayout::mono(),
            matrix: Some(rows),
        })
    };
    assert!(mix(vec![vec![0.5, 0.5]]).output_spec(&stereo).is_ok());
    let wide = mix(vec![vec![0.5, 0.5, 0.5]])
        .output_spec(&stereo)
        .unwrap_err();
    assert!(
        wide.to_string()
            .contains("mix matrix rows have 3 coefficients but the input has 2 channels"),
        "{wide}"
    );
    let tall = mix(vec![vec![1.0, 0.0], vec![0.0, 1.0]])
        .output_spec(&stereo)
        .unwrap_err();
    assert!(
        tall.to_string()
            .contains("mix matrix has 2 rows but mono has 1 channel"),
        "{tall}"
    );
}

#[test]
fn names_of_curves_and_qualities_round_trip() {
    for c in [Curve::Linear, Curve::EqualPower, Curve::SCurve] {
        assert_eq!(Curve::from_name(c.name()), Some(c));
    }
    assert_eq!(Curve::NAMES, ["linear", "equal-power", "s-curve"]);
    for q in [Quality::Fast, Quality::Good, Quality::Best] {
        assert_eq!(Quality::from_name(q.name()), Some(q));
    }
    assert_eq!(Quality::NAMES, ["fast", "good", "best"]);
    assert_eq!(
        (Curve::default(), Quality::default()),
        (Curve::Linear, Quality::Good)
    );
}

/// Passes frames through and ignores further input once `limit` frames have gone by.
struct Upto {
    spec: AudioSpec,
    seen: u64,
    limit: u64,
    held: Option<AudioFrame>,
}

impl Upto {
    fn boxed(spec: &AudioSpec, limit: u64) -> Box<dyn Filter> {
        Box::new(Self {
            spec: spec.clone(),
            seen: 0,
            limit,
            held: None,
        })
    }
}

impl Filter for Upto {
    fn output_spec(&self) -> &AudioSpec {
        &self.spec
    }
    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        self.seen += frame.frames() as u64;
        self.held = Some(frame.clone());
        Ok(())
    }
    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        match self.held.take() {
            Some(f) => {
                *out = f;
                Ok(true)
            }
            None => Ok(false),
        }
    }
    fn drain(&mut self) {}
    fn ignores_further_input(&self) -> bool {
        self.seen >= self.limit
    }
}

#[test]
fn a_chain_ignores_further_input_as_soon_as_any_stage_does() {
    let input = spec(8000, SampleFormat::F64, ChannelLayout::mono());
    let gain = || build(&gain(1.0), &input).unwrap();
    let frame = AudioFrame {
        samples: Samples::F64(vec![0.5; 4]),
        ..AudioFrame::new(&input)
    };
    for stages in [
        vec![Upto::boxed(&input, 6), gain()],
        vec![gain(), Upto::boxed(&input, 6)],
    ] {
        let mut chain =
            Chain::with_filters(&input, stages, SampleFormat::S16, Dither::None).unwrap();
        assert!(!chain.ignores_further_input());
        chain.send_frame(&frame).unwrap();
        let mut out = AudioFrame::new(chain.output_spec());
        while chain.receive_frame(&mut out).unwrap() {}
        assert!(!chain.ignores_further_input());
        chain.send_frame(&frame).unwrap();
        while chain.receive_frame(&mut out).unwrap() {}
        assert!(chain.ignores_further_input());
    }
    let plain = Chain::with_filters(&input, vec![gain()], SampleFormat::S16, Dither::None).unwrap();
    assert!(!plain.ignores_further_input());
}
