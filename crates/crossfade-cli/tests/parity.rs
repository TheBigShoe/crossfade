//! A convert made on the sheet is the convert a person would type: same
//! intent through both front doors, same `Request`.

use crossfade_app::inputs::{InputEntry, Inputs, probe_path};
use crossfade_app::sheet::{
    AdjustField, Container, ConvertSheet, FormatChoice, LevelMode, Problem,
};
use crossfade_cli::request_from_args;
use crossfade_core::{Limits, SampleFormat};
use crossfade_graph::{AudioSettings, CancelToken, Job, JobInput, JobOutput, run};
use crossfade_testkit::memory::MemoryIo;

/// A 16-bit stereo 48 kHz WAV of a few frames.
fn wav_bytes() -> Vec<u8> {
    let frames = 64u32;
    let data_len = frames * 4;
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&48_000u32.to_le_bytes());
    v.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
    v.extend_from_slice(&4u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames * 2 {
        v.extend_from_slice(&((i * 97) as i16).to_le_bytes());
    }
    v
}

/// The probed entry for `path`: a WAV, or the engine's own FLAC made from one when the
/// name ends in `.flac`.
fn entry_for(path: &str) -> InputEntry {
    let mut io = MemoryIo::new();
    if path.ends_with(".flac") {
        io.insert("seed.wav", wav_bytes());
        let job = Job {
            version: Job::VERSION,
            inputs: vec![JobInput {
                path: "seed.wav".into(),
            }],
            outputs: vec![JobOutput {
                path: path.into(),
                audio: AudioSettings::default(),
            }],
        };
        run(
            &job,
            &mut io,
            &Limits::default(),
            &CancelToken::new(),
            &mut |_| {},
        )
        .unwrap();
    } else {
        io.insert(path, wav_bytes());
    }
    let mut inputs = Inputs::new();
    inputs.add(path, probe_path(path, &mut io, &Limits::default()));
    inputs.entries()[0].clone()
}

struct Intent {
    input: &'static str,
    output: &'static str,
    format: Option<&'static str>,
    force: bool,
}

const fn intent(
    input: &'static str,
    output: &'static str,
    format: Option<&'static str>,
    force: bool,
) -> Intent {
    Intent {
        input,
        output,
        format,
        force,
    }
}

const TABLE: &[Intent] = &[
    intent("in.wav", "out.wav", None, false),
    intent("in.wav", "out.wav", Some("u8"), false),
    intent("in.wav", "out.wav", Some("s16"), false),
    intent("in.wav", "out.wav", Some("s24"), false),
    intent("in.wav", "out.wav", Some("s32"), false),
    intent("in.wav", "out.wav", Some("f32"), false),
    intent("in.wav", "out.wav", Some("f64"), false),
    intent("in.wav", "out.wav", None, true),
    intent("in.wav", "out.wav", Some("s24"), true),
    intent("my song.wav", "my song (24-bit).wav", Some("s24"), false),
    intent("it's.wav", "it's done.wav", None, true),
    intent(
        "caf\u{e9} \u{65e5}\u{672c}.wav",
        "out.wav",
        Some("f32"),
        false,
    ),
    intent("-dash.wav", "out.wav", Some("s16"), true),
    intent("in.wav", "-dash.wav", None, false),
    intent(".hidden.wav", ".out.wav", Some("f64"), false),
    intent("a.wav", "b.wav", Some("s16"), false),
];

fn format(name: &str) -> FormatChoice {
    FormatChoice::Format(SampleFormat::from_name(name).unwrap())
}

// What a person would type; a dash-leading input has to sit behind `--`.
fn from_cli(i: &Intent) -> crossfade_app::Request {
    let mut flags = vec![format!("--output={}", i.output)];
    if let Some(f) = i.format {
        flags.extend(["--sample-format".into(), f.into()]);
    }
    if i.force {
        flags.push("--force".into());
    }
    let mut args = vec!["convert".to_owned()];
    if i.input.starts_with('-') {
        args.extend(flags);
        args.extend(["--".into(), i.input.into()]);
    } else {
        args.push(i.input.into());
        args.extend(flags);
    }
    request_from_args(&args).unwrap().unwrap()
}

fn sheet_for(path: &str) -> ConvertSheet {
    ConvertSheet::for_input(&entry_for(path))
}

fn from_sheet(i: &Intent) -> crossfade_app::Request {
    let mut sheet = sheet_for(i.input);
    sheet.set_output(i.output.into());
    if let Some(f) = i.format {
        sheet.set_format(format(f));
    }
    sheet.set_overwrite(i.force);
    sheet.build(&|_| false).unwrap()
}

#[test]
fn every_intent_builds_the_same_request_on_both_paths() {
    for i in TABLE {
        assert_eq!(from_sheet(i), from_cli(i), "{} -> {}", i.input, i.output);
    }
}

#[test]
fn each_preset_matches_its_sample_format_flag() {
    for (preset, flag) in [("cd", "s16"), ("studio", "s24"), ("float", "f32")] {
        let mut sheet = sheet_for("in.wav");
        sheet.set_output("out.wav".into());
        sheet.apply_preset(preset);
        let typed = request_from_args([
            "convert",
            "in.wav",
            "-o",
            "out.wav",
            "--sample-format",
            flag,
        ])
        .unwrap()
        .unwrap();
        assert_eq!(sheet.build(&|_| false).unwrap(), typed, "{preset}");
    }
    let mut sheet = sheet_for("in.wav");
    sheet.set_output("out.wav".into());
    sheet.apply_preset("same");
    let typed = request_from_args(["convert", "in.wav", "-o", "out.wav"])
        .unwrap()
        .unwrap();
    assert_eq!(sheet.build(&|_| false).unwrap(), typed);
}

#[test]
fn the_default_output_is_what_a_person_would_type() {
    let mut sheet = sheet_for("take 1.wav");
    sheet.set_format(format("s24"));
    let typed = request_from_args([
        "convert",
        "take 1.wav",
        "-o",
        "take 1-s24.wav",
        "--sample-format",
        "s24",
    ])
    .unwrap()
    .unwrap();
    assert_eq!(sheet.build(&|_| false).unwrap(), typed);
}

/// One thing a person does on the sheet.
#[derive(Clone, Copy)]
enum Do {
    Container(Container),
    Preset(&'static str),
    Format(&'static str),
    Level(Option<u8>),
    Copy,
    Output(&'static str),
}

struct Row {
    input: &'static str,
    steps: &'static [Do],
    /// What they would type after `convert <input>`.
    cli: &'static [&'static str],
}

const ROWS: &[Row] = &[
    // FLAC output: by container, by each preset, with levels.
    Row {
        input: "in.wav",
        steps: &[Do::Container(Container::Flac)],
        cli: &["-o", "in.flac"],
    },
    Row {
        input: "in.wav",
        steps: &[Do::Preset("flac")],
        cli: &["-o", "in.flac"],
    },
    Row {
        input: "in.wav",
        steps: &[Do::Preset("flac-cd")],
        cli: &["-o", "in-s16.flac", "--sample-format", "s16"],
    },
    Row {
        input: "in.wav",
        steps: &[Do::Preset("flac-24")],
        cli: &["-o", "in-s24.flac", "--sample-format", "s24"],
    },
    Row {
        input: "in.wav",
        steps: &[Do::Container(Container::Flac), Do::Format("s32")],
        cli: &["-o", "in-s32.flac", "--sample-format", "s32"],
    },
    Row {
        input: "in.wav",
        steps: &[Do::Container(Container::Flac), Do::Level(Some(0))],
        cli: &["-o", "in.flac", "--compression-level", "0"],
    },
    Row {
        input: "in.wav",
        steps: &[Do::Container(Container::Flac), Do::Level(Some(8))],
        cli: &["-o", "in.flac", "--compression-level", "8"],
    },
    Row {
        input: "in.wav",
        steps: &[Do::Output("out.flac"), Do::Format("u8"), Do::Level(Some(5))],
        cli: &[
            "-o",
            "out.flac",
            "--sample-format",
            "u8",
            "--compression-level",
            "5",
        ],
    },
    // AIFF output.
    Row {
        input: "in.wav",
        steps: &[Do::Preset("aiff")],
        cli: &["-o", "in.aiff"],
    },
    Row {
        input: "in.wav",
        steps: &[Do::Container(Container::Aiff), Do::Format("s24")],
        cli: &["-o", "in-s24.aiff", "--sample-format", "s24"],
    },
    // Ogg FLAC output.
    Row {
        input: "in.wav",
        steps: &[Do::Container(Container::OggFlac)],
        cli: &["-o", "in.oga"],
    },
    Row {
        input: "in.wav",
        steps: &[Do::Container(Container::OggFlac), Do::Level(Some(8))],
        cli: &["-o", "in.oga", "--compression-level", "8"],
    },
    Row {
        input: "in.wav",
        steps: &[Do::Container(Container::OggFlac), Do::Format("s24")],
        cli: &["-o", "in-s24.oga", "--sample-format", "s24"],
    },
    Row {
        input: "in.wav",
        steps: &[Do::Output("typed.ogg"), Do::Level(Some(3))],
        cli: &["-o", "typed.ogg", "--compression-level", "3"],
    },
    Row {
        input: "in.flac",
        steps: &[Do::Container(Container::OggFlac), Do::Copy],
        cli: &["-o", "in.oga", "--codec", "copy"],
    },
    // Stream copy.
    Row {
        input: "in.flac",
        steps: &[Do::Container(Container::Flac), Do::Copy],
        cli: &["-o", "in-copy.flac", "--codec", "copy"],
    },
    Row {
        input: "in.flac",
        steps: &[Do::Output("out.flac"), Do::Copy],
        cli: &["-o", "out.flac", "--codec", "copy"],
    },
    Row {
        input: "in.wav",
        steps: &[Do::Copy],
        cli: &["-o", "in-copy.wav", "--codec", "copy"],
    },
    // A container chosen after a path was typed changes only the extension.
    Row {
        input: "in.wav",
        steps: &[Do::Output("my mix.wav"), Do::Container(Container::Flac)],
        cli: &["-o", "my mix.flac"],
    },
    Row {
        input: "in.wav",
        steps: &[
            Do::Output("my mix.wav"),
            Do::Container(Container::Flac),
            Do::Level(Some(2)),
            Do::Container(Container::Aiff),
        ],
        cli: &["-o", "my mix.aiff"],
    },
    // A typed extension picks the container.
    Row {
        input: "in.wav",
        steps: &[Do::Output("typed.flac")],
        cli: &["-o", "typed.flac"],
    },
];

#[test]
fn flac_aiff_and_copy_intents_build_what_the_cli_builds() {
    for row in ROWS {
        let mut sheet = sheet_for(row.input);
        for step in row.steps {
            match *step {
                Do::Container(c) => sheet.set_container(c),
                Do::Preset(id) => sheet.apply_preset(id),
                Do::Format(f) => sheet.set_format(format(f)),
                Do::Level(l) => sheet.set_level(l),
                Do::Copy => sheet.set_copy(true),
                Do::Output(path) => sheet.set_output(path.into()),
            }
        }
        let mut args = vec!["convert", row.input];
        args.extend(row.cli);
        let typed = request_from_args(&args).unwrap().unwrap();
        assert_eq!(
            sheet.build(&|_| false).unwrap(),
            typed,
            "{} {:?}",
            row.input,
            row.cli
        );
    }
}

/// Processing typed with shortcuts or filters renders back (every filter as `--filter`, in
/// the job's order) to a command line that parses to the same request. The sheet's controls
/// are held to the shortcut flags in `processing_controls_build_what_the_shortcut_flags_build`.
#[test]
fn processing_intents_render_back_to_the_same_request() {
    let table: &[&[&str]] = &[
        &["convert", "in.wav", "-o", "out.flac", "--gain", "-6dB"],
        &[
            "convert",
            "in.wav",
            "-o",
            "out.flac",
            "--filter",
            "gain -6dB",
        ],
        &[
            "convert",
            "in.wav",
            "-o",
            "out.wav",
            "--start",
            "1:30",
            "--fade-out",
            "3s",
            "--filter",
            "mix mono",
            "--normalize",
            "-16LUFS",
            "--true-peak",
            "-2dBTP",
        ],
        &[
            "convert",
            "my song.wav",
            "-o",
            "out.flac",
            "--sample-rate",
            "44.1kHz",
            "--channels",
            "stereo",
            "--dither",
            "none",
            "--sample-format",
            "s16",
        ],
        &[
            "convert",
            "in.wav",
            "-o",
            "out.flac",
            "--filter",
            "resample 96kHz quality=best",
            "--filter",
            "mix 2 matrix=1,0.5/0,0.5",
            "--force",
        ],
        &[
            "concat",
            "a.wav",
            "b.flac",
            "-o",
            "ab.flac",
            "--fade-in",
            "500ms",
        ],
        &["concat", "--output=x.wav", "--", "-a.wav", "b.wav"],
    ];
    for typed in table {
        let request = request_from_args(typed.iter().copied()).unwrap().unwrap();
        let rendered = crossfade_app::command_args(&request).unwrap();
        assert_eq!(
            request_from_args(&rendered).unwrap(),
            Some(request),
            "{typed:?} -> {rendered:?}"
        );
    }
}

enum Adj {
    Rate(u32),
    Channels(&'static str),
    Level(LevelMode, &'static str),
    Text(AdjustField, &'static str),
    Format(&'static str),
    Container(Container),
    Open,
}

struct Processing {
    steps: &'static [Adj],
    cli: &'static [&'static str],
}

const PROCESSING: &[Processing] = &[
    Processing {
        steps: &[Adj::Rate(44_100)],
        cli: &["--sample-rate", "44.1kHz"],
    },
    Processing {
        steps: &[Adj::Rate(96_000)],
        cli: &["--sample-rate", "96kHz"],
    },
    Processing {
        steps: &[Adj::Channels("mono")],
        cli: &["--channels", "mono"],
    },
    Processing {
        steps: &[Adj::Level(LevelMode::Gain, "-3 dB")],
        cli: &["--gain", "-3dB"],
    },
    Processing {
        steps: &[Adj::Level(LevelMode::Gain, "3")],
        cli: &["--gain", "3dB"],
    },
    Processing {
        steps: &[Adj::Level(LevelMode::Normalize, "-16")],
        cli: &["--normalize", "-16LUFS"],
    },
    Processing {
        steps: &[Adj::Level(LevelMode::Normalize, "-16 LUFS")],
        cli: &["--normalize", "-16LUFS", "--true-peak", "-1dBTP"],
    },
    Processing {
        steps: &[Adj::Text(AdjustField::Start, "1:30")],
        cli: &["--start", "1:30"],
    },
    Processing {
        steps: &[Adj::Text(AdjustField::End, "2:00")],
        cli: &["--end", "2:00"],
    },
    Processing {
        steps: &[
            Adj::Text(AdjustField::Start, "90"),
            Adj::Text(AdjustField::End, "120000ms"),
        ],
        cli: &["--start", "90s", "--end", "120000ms"],
    },
    Processing {
        steps: &[Adj::Text(AdjustField::FadeIn, "500ms")],
        cli: &["--fade-in", "500ms"],
    },
    Processing {
        steps: &[Adj::Text(AdjustField::FadeOut, "3")],
        cli: &["--fade-out", "3s"],
    },
    Processing {
        steps: &[
            Adj::Text(AdjustField::FadeOut, "3"),
            Adj::Text(AdjustField::FadeIn, "1.5"),
        ],
        cli: &["--fade-in", "1.5s", "--fade-out", "3s"],
    },
    Processing {
        steps: &[
            Adj::Open,
            Adj::Rate(48_000),
            Adj::Channels("mono"),
            Adj::Level(LevelMode::Normalize, "-16"),
            Adj::Text(AdjustField::FadeOut, "3"),
            Adj::Text(AdjustField::End, "1:30"),
            Adj::Text(AdjustField::Start, "0:10"),
            Adj::Text(AdjustField::FadeIn, "1"),
        ],
        cli: &[
            "--sample-rate",
            "48kHz",
            "--channels",
            "mono",
            "--start",
            "0:10",
            "--end",
            "1:30",
            "--fade-in",
            "1s",
            "--fade-out",
            "3s",
            "--normalize",
            "-16LUFS",
        ],
    },
    Processing {
        steps: &[
            Adj::Level(LevelMode::Gain, "-1.5"),
            Adj::Text(AdjustField::Start, "5"),
            Adj::Text(AdjustField::FadeOut, "2"),
        ],
        cli: &["--start", "5s", "--fade-out", "2s", "--gain", "-1.5dB"],
    },
    Processing {
        steps: &[
            Adj::Container(Container::Flac),
            Adj::Format("s16"),
            Adj::Channels("mono"),
            Adj::Text(AdjustField::FadeOut, "3"),
        ],
        cli: &[
            "--sample-format",
            "s16",
            "--channels",
            "mono",
            "--fade-out",
            "3s",
        ],
    },
];

/// Each new control alone and several together build the request the matching flags build.
/// `draft` is compared, so a filter the engine has not built yet cannot fail the test; when
/// `build` does refuse, it is the engine's word, not a problem of the sheet's own.
#[test]
fn processing_controls_build_what_the_shortcut_flags_build() {
    for case in PROCESSING {
        let mut sheet = sheet_for("in.wav");
        let mut container = Container::Wav;
        for step in case.steps {
            match step {
                Adj::Rate(r) => sheet.set_sample_rate(Some(*r)),
                Adj::Channels(name) => {
                    sheet.set_channels(Some(crossfade_graph::filter::parse_channels(name).unwrap()))
                }
                Adj::Level(mode, text) => {
                    sheet.set_level_mode(*mode);
                    sheet.set_adjust_text(AdjustField::Level, (*text).into());
                }
                Adj::Text(field, text) => sheet.set_adjust_text(*field, (*text).into()),
                Adj::Format(f) => sheet.set_format(format(f)),
                Adj::Container(c) => {
                    container = *c;
                    sheet.set_container(*c);
                }
                Adj::Open => sheet.set_adjust_open(true),
            }
        }
        let mut args = vec!["convert".to_owned(), "in.wav".into()];
        let output = sheet.output().to_owned();
        args.extend(["-o".to_owned(), output]);
        args.extend(case.cli.iter().map(|a| (*a).to_owned()));
        let typed = request_from_args(&args).unwrap().unwrap();
        assert_eq!(
            sheet.draft(),
            typed,
            "{:?} {:?}",
            container.label(),
            case.cli
        );
        match sheet.build(&|_| false) {
            Ok(built) => assert_eq!(built, typed, "{:?}", case.cli),
            Err(problems) => assert!(
                problems
                    .iter()
                    .all(|p| matches!(p, Problem::Source(text) if text.contains("not built"))),
                "{:?}: {problems:?}",
                case.cli
            ),
        }
        // And it renders back to a command that parses to the same request.
        let rendered = crossfade_app::command_args(&typed).unwrap();
        assert_eq!(request_from_args(&rendered).unwrap(), Some(typed));
    }
}

/// Typing a sheet value the engine refuses is what the flag refuses.
#[test]
fn what_the_sheet_refuses_the_command_line_refuses_too() {
    let mut sheet = sheet_for("in.wav");
    sheet.set_adjust_text(AdjustField::Start, "30".into());
    sheet.set_adjust_text(AdjustField::End, "10".into());
    assert!(sheet.build(&|_| false).is_err());
    let args = [
        "convert", "in.wav", "-o", "out.wav", "--start", "30s", "--end", "10s",
    ];
    assert!(request_from_args(args).is_err());
}
