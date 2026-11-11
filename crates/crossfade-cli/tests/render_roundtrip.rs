//! Any job the engine can hold, filters and several inputs included, renders to arguments
//! (and a shell line) that the CLI parses back to the same request.

use crossfade_app::{Request, command_args, command_line};
use crossfade_cli::request_from_args;
use crossfade_core::ChannelLayout;
use crossfade_graph::filter::{Channels, Curve, FilterSpec, Quality};
use crossfade_graph::{AudioSettings, Dither, Job, JobInput, JobOutput};
use proptest::prelude::*;

fn path() -> impl Strategy<Value = String> {
    let prefix = prop::sample::select(vec!["", "", "-", "--", "."]);
    (prefix, "[a-z0-9 '.]{0,8}").prop_map(|(p, s)| format!("{p}{s}.wav"))
}

fn seconds() -> impl Strategy<Value = f64> {
    prop_oneof![
        (0u32..600_000).prop_map(|ms| f64::from(ms) / 1000.0),
        0.0f64..1e5
    ]
}

fn filter() -> impl Strategy<Value = FilterSpec> {
    let channels = prop_oneof![
        (1u16..=8).prop_map(Channels::Count),
        prop::sample::select(ChannelLayout::NAMES.to_vec())
            .prop_map(|n| Channels::Layout(ChannelLayout::from_name(n).unwrap())),
    ];
    prop_oneof![
        (-120.0f64..=60.0).prop_map(|db| FilterSpec::Gain { db }),
        (seconds(), 0.001f64..1e4, any::<bool>()).prop_map(|(start, len, by_end)| {
            FilterSpec::Trim {
                start_seconds: Some(start),
                end_seconds: by_end.then_some(start + len),
                duration_seconds: (!by_end).then_some(len),
            }
        }),
        (
            0.001f64..100.0,
            prop::option::of(0.001f64..100.0),
            prop::sample::select(vec![Curve::Linear, Curve::EqualPower, Curve::SCurve])
        )
            .prop_map(|(i, o, curve)| FilterSpec::Fade {
                in_seconds: Some(i),
                out_seconds: o,
                curve,
            }),
        (-70.0f64..=0.0, -60.0f64..=0.0).prop_map(|(lufs, true_peak_dbtp)| {
            FilterSpec::Normalize {
                lufs,
                true_peak_dbtp,
            }
        }),
        (
            1u32..=1_048_575,
            prop::sample::select(vec![Quality::Fast, Quality::Good, Quality::Best])
        )
            .prop_map(|(rate, quality)| FilterSpec::Resample { rate, quality }),
        (channels, any::<bool>(), -16.0f64..=16.0).prop_map(|(channels, with, c)| {
            let rows = usize::from(match &channels {
                Channels::Count(n) => *n,
                Channels::Layout(l) => l.channels(),
            });
            FilterSpec::Mix {
                matrix: with.then(|| vec![vec![c, -c]; rows]),
                channels,
            }
        }),
    ]
}

fn request() -> impl Strategy<Value = Request> {
    (
        prop::collection::vec(path(), 1..4),
        path(),
        prop::option::of(1u32..=1_048_575),
        prop::option::of((1u16..=64).prop_map(Channels::Count)),
        any::<bool>(),
        prop::collection::vec(filter(), 0..4),
        any::<bool>(),
    )
        .prop_map(
            |(inputs, output, sample_rate, channels, dither, filters, overwrite)| Request {
                job: Job {
                    version: Job::VERSION,
                    inputs: inputs.into_iter().map(|path| JobInput { path }).collect(),
                    outputs: vec![JobOutput {
                        path: output,
                        audio: AudioSettings {
                            sample_rate,
                            channels,
                            dither: if dither { Dither::None } else { Dither::Tpdf },
                            filters,
                            ..AudioSettings::default()
                        },
                    }],
                },
                overwrite,
            },
        )
}

/// Single quotes and `'\''`, which is all `command_line` writes.
fn split_posix(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let (mut cur, mut in_word, mut quoted, mut escaped) = (String::new(), false, false, false);
    for c in line.chars() {
        if escaped {
            cur.push(c);
            escaped = false;
            continue;
        }
        match (quoted, c) {
            (false, '\\') => (escaped, in_word) = (true, true),
            (true, '\'') => quoted = false,
            (true, c) => cur.push(c),
            (false, '\'') => (quoted, in_word) = (true, true),
            (false, ' ') if in_word => {
                words.push(std::mem::take(&mut cur));
                in_word = false;
            }
            (false, ' ') => {}
            (false, c) => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    words
}

proptest! {
    #[test]
    fn the_cli_parses_back_what_command_args_prints(r in request()) {
        let args = command_args(&r).unwrap();
        prop_assert_eq!(request_from_args(&args).unwrap(), Some(r));
    }

    #[test]
    fn command_line_splits_back_into_command_args(r in request()) {
        let words = split_posix(&command_line(&r).unwrap());
        prop_assert_eq!(&words[0], "crossfade");
        let args = command_args(&r).unwrap();
        prop_assert_eq!(&words[1..], args.as_slice());
    }
}
