//! The resampler on several threads: the stream is cut into segments that start on a
//! multiple of the ratio's input period, each converted by its own resampler from a pre-roll
//! as long as the filter reaches back, so every output is computed from the same inputs with
//! the same coefficients in the same order. The output is the one-thread output, bit for bit.

use crossfade_core::{AudioFrame, AudioSpec, ChannelLayout, Filter, SampleFormat, Samples};
use crossfade_dsp::chain::{Chain, ChainPlan};
use crossfade_dsp::dither::Dither;
use crossfade_dsp::filters::FilterPlan;
use crossfade_dsp::filters::resample::{Quality, Resample, ResamplePlan};
use proptest::prelude::*;

fn spec(rate: u32, channels: u16) -> AudioSpec {
    AudioSpec::new(
        rate,
        SampleFormat::F64,
        ChannelLayout::default_for(channels).unwrap(),
    )
    .unwrap()
}

fn signal(frames: usize, channels: usize, seed: u64) -> Vec<f64> {
    let mut s = seed | 1;
    (0..frames * channels)
        .map(|i| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            0.6 * libm::sin(i as f64 * 0.0137)
                + ((s >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 0.3
        })
        .collect()
}

/// Feeds `x` in `chunk`-frame frames, drains, returns every sample out and the pts seen.
fn run(f: &mut Resample, input: &AudioSpec, x: &[f64], chunk: usize) -> (Vec<f64>, Vec<i64>) {
    let ch = usize::from(input.layout().channels());
    let mut out = AudioFrame::new(f.output_spec());
    let (mut all, mut pts) = (Vec::new(), Vec::new());
    let mut collect = |f: &mut Resample, out: &mut AudioFrame| {
        while f.receive_frame(out).unwrap() {
            assert!(!out.samples.is_empty());
            let Samples::F64(s) = &out.samples else {
                panic!()
            };
            pts.push(out.pts);
            all.extend_from_slice(s);
        }
    };
    let mut at = 0;
    for part in x.chunks((chunk * ch).max(ch)) {
        let frame = AudioFrame {
            pts: at,
            samples: Samples::F64(part.to_vec()),
            ..AudioFrame::new(input)
        };
        at += (part.len() / ch) as i64;
        f.send_frame(&frame).unwrap();
        collect(f, &mut out);
    }
    f.drain();
    collect(f, &mut out);
    (all, pts)
}

const PAIRS: [(u32, u32); 9] = [
    (44_100, 48_000),
    (48_000, 44_100),
    (44_100, 44_101),
    (8_000, 192_000),
    (192_000, 8_000),
    (96_000, 44_100),
    (22_050, 44_100),
    (11_025, 8_000),
    (44_100, 1_000),
];

fn plan(rate: u32, quality: Quality) -> ResamplePlan {
    ResamplePlan { rate, quality }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn every_thread_count_gives_the_one_thread_bits(
        pair in 0..PAIRS.len(),
        channels in 1u16..=6,
        quality in prop::sample::select(vec![Quality::Fast, Quality::Good, Quality::Best]),
        frames in 0usize..30_000,
        chunk in 1usize..6_000,
        threads in 2usize..=5,
        segment in 1u64..3_000,
        seed in any::<u64>(),
    ) {
        let (from, to) = PAIRS[pair];
        let input = spec(from, channels);
        let x = signal(frames, usize::from(channels), seed);
        let mut one = Resample::with_threads(&input, &plan(to, quality), 1).unwrap();
        let (want, want_pts) = run(&mut one, &input, &x, chunk);
        let mut many = Resample::with_segment(&input, &plan(to, quality), threads, segment).unwrap();
        let (got, got_pts) = run(&mut many, &input, &x, chunk);
        prop_assert_eq!(got.len(), want.len());
        prop_assert!(got.iter().zip(&want).all(|(a, b)| a.to_bits() == b.to_bits()),
            "{} -> {} x{} {:?}", from, to, channels, quality);
        // Both start at 0; the framing may differ.
        prop_assert_eq!(got_pts.first().copied().unwrap_or(0), 0);
        prop_assert_eq!(want_pts.first().copied().unwrap_or(0), 0);
    }
}

#[test]
fn one_thread_and_short_inputs_start_none() {
    let input = spec(44_100, 2);
    let x = signal(20_000, 2, 7);
    let mut one = Resample::with_threads(&input, &plan(48_000, Quality::Good), 1).unwrap();
    run(&mut one, &input, &x, 4096);
    assert_eq!(one.parallel_batches(), 0);
    // Shorter than two segments: converted on the caller's thread.
    let mut short = Resample::with_threads(&input, &plan(48_000, Quality::Good), 8).unwrap();
    run(&mut short, &input, &x, 4096);
    assert_eq!(short.parallel_batches(), 0);
    let long = signal(600_000, 2, 9);
    let mut many = Resample::with_threads(&input, &plan(48_000, Quality::Good), 4).unwrap();
    run(&mut many, &input, &long, 4096);
    assert!(many.parallel_batches() > 0);
}

#[test]
fn a_long_stream_through_a_threaded_chain_matches_one_thread() {
    let input = AudioSpec::new(
        96_000,
        SampleFormat::S24,
        ChannelLayout::from_name("5.1").unwrap(),
    )
    .unwrap();
    let samples: Vec<i32> = (0..6 * 400_000)
        .map(|i| ((i as i64 * 2_654_435_761) % 8_000_000) as i32)
        .collect();
    let plan = ChainPlan {
        filters: vec![FilterPlan::Resample(ResamplePlan {
            rate: 44_100,
            quality: Quality::Good,
        })],
        dither: Dither::Tpdf,
    };
    let convert = |threads| {
        let mut chain = Chain::with_threads(&input, &plan, SampleFormat::S24, threads).unwrap();
        let mut out = AudioFrame::new(chain.output_spec());
        let mut all = Vec::new();
        for (k, part) in samples.chunks(6 * 4096).enumerate() {
            let frame = AudioFrame {
                pts: (k * 4096) as i64,
                samples: Samples::S24(part.to_vec()),
                ..AudioFrame::new(&input)
            };
            chain.send_frame(&frame).unwrap();
            while chain.receive_frame(&mut out).unwrap() {
                all.push(out.samples.clone());
            }
        }
        chain.drain();
        while chain.receive_frame(&mut out).unwrap() {
            all.push(out.samples.clone());
        }
        all.into_iter()
            .flat_map(|s| match s {
                Samples::S24(v) => v,
                _ => unreachable!(),
            })
            .collect::<Vec<i32>>()
    };
    let one = convert(1);
    assert_eq!(one.len(), 6 * 183_750);
    assert_eq!(convert(3), one);
    assert_eq!(convert(0), one, "0 is the machine's parallelism");
}
