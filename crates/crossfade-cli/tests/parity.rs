//! A convert made on the sheet is the convert a person would type: same
//! intent through both front doors, same `Request`.

use crossfade_app::inputs::{InputState, Inputs};
use crossfade_app::sheet::{ConvertSheet, FormatChoice};
use crossfade_cli::request_from_args;
use crossfade_core::SampleFormat;

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
    let mut inputs = Inputs::new();
    inputs.add(path, InputState::Failed(String::new()));
    ConvertSheet::for_input(&inputs.entries()[0])
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
