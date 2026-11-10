//! The arguments a front end shows are exactly the arguments the CLI parses.

use crossfade_app::{Request, command_line, convert_args};
use crossfade_cli::request_from_args;
use crossfade_core::SampleFormat;
use crossfade_graph::{AudioSettings, Job, JobInput, JobOutput};
use proptest::prelude::*;

const FORMATS: [SampleFormat; 6] = [
    SampleFormat::U8,
    SampleFormat::S16,
    SampleFormat::S24,
    SampleFormat::S32,
    SampleFormat::F32,
    SampleFormat::F64,
];

fn path() -> impl Strategy<Value = String> {
    let chars: Vec<char> = "abcXYZ019 '\"`$\\*?;&|()<>#~.-_/\u{e9}\u{65e5}\u{1f3b5}\t\n"
        .chars()
        .collect();
    // Leading dashes and dots are the awkward cases, so make them common.
    let prefix = prop::sample::select(vec!["", "", "", "-", "--", "-o", "."]);
    (
        prefix,
        prop::collection::vec(prop::sample::select(chars), 0..10),
    )
        .prop_map(|(p, v)| format!("{p}{}", v.into_iter().collect::<String>()))
}

fn request() -> impl Strategy<Value = Request> {
    (
        path(),
        path(),
        prop::option::of(prop::sample::select(FORMATS.to_vec())),
        any::<bool>(),
    )
        .prop_map(|(input, output, format, overwrite)| Request {
            job: Job {
                version: Job::VERSION,
                inputs: vec![JobInput { path: input }],
                outputs: vec![JobOutput {
                    path: output,
                    audio: AudioSettings {
                        sample_format: format,
                    },
                }],
            },
            overwrite,
        })
}

/// Just enough of a POSIX shell: words, single quotes and `\\'`.
fn split_posix(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quoted = false;
    let mut escaped = false;
    for c in line.chars() {
        if escaped {
            cur.push(c);
            escaped = false;
            continue;
        }
        match (quoted, c) {
            (false, '\\') => {
                escaped = true;
                in_word = true;
            }
            (true, '\'') => quoted = false,
            (true, c) => cur.push(c),
            (false, '\'') => {
                quoted = true;
                in_word = true;
            }
            (false, ' ') => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            (false, c) => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    assert!(!quoted, "unterminated quote in {line:?}");
    if in_word {
        words.push(cur);
    }
    words
}

proptest! {
    #[test]
    fn the_cli_parses_back_what_convert_args_prints(r in request()) {
        let args = convert_args(&r).unwrap();
        prop_assert_eq!(request_from_args(&args).unwrap(), Some(r));
    }

    #[test]
    fn command_line_splits_back_into_convert_args(r in request()) {
        let words = split_posix(&command_line(&r).unwrap());
        prop_assert_eq!(&words[0], "crossfade");
        let args = convert_args(&r).unwrap();
        prop_assert_eq!(&words[1..], args.as_slice());
    }
}

#[test]
fn other_subcommands_are_not_requests() {
    assert_eq!(request_from_args(["probe", "a.wav"]).unwrap(), None);
}

#[test]
fn bad_arguments_are_a_clap_error() {
    assert!(request_from_args(["convert", "a.wav"]).is_err());
}
