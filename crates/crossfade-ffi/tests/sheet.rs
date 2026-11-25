//! A convert sheet driven only by JSON changes through the C functions builds the request the
//! command line builds for the same intent, with the same notes.
#![allow(clippy::undocumented_unsafe_blocks)]

mod common;

use common::{Dir, c, fixture, json_out, last_error, write_wav};
use crossfade_app::FsIo;
use crossfade_app::sheet::plain_note;
use crossfade_cli::{Outcome, request_from_args, run_command};
use crossfade_ffi::*;
use crossfade_graph::{CancelToken, Event, Job};
use serde_json::{Value, json};

/// A sheet handle a test owns, destroyed when dropped.
struct Sheet(*mut CfSheet);

impl Sheet {
    fn open(path: &str) -> Self {
        let path = c(path);
        let mut handle = std::ptr::null_mut();
        let status = unsafe { cf_sheet_new(path.as_ptr(), &mut handle) };
        assert_eq!(status, CF_OK, "{:?}", last_error());
        Self(handle)
    }

    fn change(&self, changes: &Value) -> i32 {
        let text = c(&changes.to_string());
        unsafe { cf_sheet_apply(self.0, text.as_ptr()) }
    }

    fn state(&self) -> Value {
        json_out(|out| unsafe { cf_sheet_state(self.0, out) })
    }
}

impl Drop for Sheet {
    fn drop(&mut self) {
        unsafe { cf_sheet_destroy(self.0) };
    }
}

/// The request in the state, as the engine's own types.
fn request_of(state: &Value) -> crossfade_app::Request {
    let job: Job = serde_json::from_value(state["request"]["job"].clone()).unwrap();
    crossfade_app::Request {
        job,
        overwrite: state["request"]["overwrite"].as_bool().unwrap(),
    }
}

/// The notes the command line prints for `args`, in the sheet's words, from a real run into
/// `cli_out`.
fn cli_notes(args: &[String]) -> Vec<String> {
    let mut notes = Vec::new();
    let outcome = run_command(
        args,
        &mut FsIo::new(),
        &CancelToken::new(),
        &mut |e: &Event| {
            if let Event::Started { plan } = e {
                notes.extend(plan.notes.iter().map(|n| plain_note(n)));
            }
        },
    )
    .unwrap();
    assert!(matches!(outcome, Outcome::Done(_)), "{outcome:?}");
    notes
}

struct Intent {
    input: &'static str,
    changes: Value,
    output: &'static str,
    cli: &'static [&'static str],
}

fn intents() -> Vec<Intent> {
    let i = |input, changes, output, cli| Intent {
        input,
        changes,
        output,
        cli,
    };
    vec![
        i(
            "in48.wav",
            json!([{"output": "@/out.flac"}, {"compression_level": 8}]),
            "out.flac",
            &["--compression-level", "8"],
        ),
        i(
            "in48.wav",
            json!([{"output": "@/out.wav"}, {"format": "s24"}]),
            "out.wav",
            &["--sample-format", "s24"],
        ),
        i(
            "in48.wav",
            json!([{"container": "m4a"}, {"output": "@/out.m4a"}]),
            "out.m4a",
            &[],
        ),
        i(
            "in44.wav",
            json!([{"container": "opus"}, {"output": "@/out.opus"}, {"bitrate": 96000}]),
            "out.opus",
            &["--bitrate", "96k"],
        ),
        i(
            "in48.wav",
            json!([{"output": "@/out.wav"}, {"trim_start": "0.5"}, {"trim_end": "1.5"}]),
            "out.wav",
            &["--start", "0.5s", "--end", "1.5s"],
        ),
        i(
            "in48.wav",
            json!([{"output": "@/out.flac"}, {"fade_in": "0.25"}, {"fade_out": "500ms"}]),
            "out.flac",
            &["--fade-in", "0.25s", "--fade-out", "500ms"],
        ),
        i(
            "in48.wav",
            json!([{"output": "@/out.wav"}, {"level_mode": "gain"}, {"level_value": "-3"}]),
            "out.wav",
            &["--gain", "-3dB"],
        ),
        i(
            "in48.wav",
            json!([{"output": "@/out.flac"}, {"adjust_open": true}, {"level_mode": "normalize"}]),
            "out.flac",
            &["--normalize", "-16LUFS"],
        ),
        i(
            "in48.wav",
            json!([{"output": "@/out.flac"}, {"sample_rate": 44100}, {"channels": "mono"}]),
            "out.flac",
            &["--sample-rate", "44.1kHz", "--channels", "mono"],
        ),
        i("in.mp3", json!([{"output": "@/out.wav"}]), "out.wav", &[]),
        i(
            "in48.wav",
            json!([{"output": "@/out.wav"}, {"overwrite": true}]),
            "out.wav",
            &["--force"],
        ),
        // One object: its keys go in the documented order, container before level.
        i(
            "in48.wav",
            json!({"compression_level": 5, "container": "flac"}),
            "in48.flac",
            &["--compression-level", "5"],
        ),
    ]
}

/// `@` in a change's text stands for the test's directory.
fn place(v: &Value, dir: &str) -> Value {
    match v {
        Value::String(s) => Value::String(s.replace('@', dir)),
        Value::Array(a) => Value::Array(a.iter().map(|x| place(x, dir)).collect()),
        Value::Object(o) => {
            Value::Object(o.iter().map(|(k, x)| (k.clone(), place(x, dir))).collect())
        }
        other => other.clone(),
    }
}

fn inputs(dir: &Dir) {
    write_wav(dir, "in48.wav", 48_000, 2, 96_000);
    write_wav(dir, "in44.wav", 44_100, 2, 88_200);
    std::fs::copy(fixture("mp3/cbr128.mp3"), dir.path("in.mp3")).unwrap();
}

#[test]
fn json_changes_build_the_command_lines_request_and_notes() {
    let dir = Dir::new("sheet-parity");
    let cli_dir = Dir::new("sheet-parity-cli");
    inputs(&dir);
    let root = dir.0.to_string_lossy().into_owned();
    for intent in intents() {
        let input = dir.path(intent.input);
        let sheet = Sheet::open(&input);
        let status = sheet.change(&place(&intent.changes, &root));
        assert_eq!(status, CF_OK, "{:?}: {:?}", intent.cli, last_error());
        let state = sheet.state();

        let output = dir.path(intent.output);
        let mut args = vec!["convert".to_owned(), input.clone(), "-o".into(), output];
        args.extend(intent.cli.iter().map(|a| (*a).to_owned()));
        let typed = request_from_args(&args).unwrap().unwrap();
        assert_eq!(request_of(&state), typed, "{:?}", intent.cli);
        assert_eq!(
            state["can_queue"], true,
            "{:?}: {}",
            intent.cli, state["problems"]
        );
        assert_eq!(
            state["command_line"],
            crossfade_app::command_line(&typed).unwrap(),
        );

        let mut run_args = args.clone();
        run_args[3] = cli_dir.path(intent.output);
        let _ = std::fs::remove_file(&run_args[3]);
        let notes: Vec<String> = serde_json::from_value(state["notes"].clone()).unwrap();
        assert_eq!(notes, cli_notes(&run_args), "{:?}", intent.cli);
    }
}

#[test]
fn the_spread_of_intents_includes_notes() {
    let dir = Dir::new("sheet-notes");
    inputs(&dir);
    let mp3 = Sheet::open(&dir.path("in.mp3"));
    let notes = mp3.state()["notes"].clone();
    assert_eq!(
        notes,
        json!(["MP3 has no bit depth: writing 16-bit; choose a Format to change it"])
    );
    let opus = Sheet::open(&dir.path("in44.wav"));
    assert_eq!(opus.change(&json!({"container": "opus"})), CF_OK);
    assert!(!opus.state()["notes"].as_array().unwrap().is_empty());
}

#[test]
fn a_refused_combination_names_its_problem_and_cannot_be_queued() {
    let dir = Dir::new("sheet-refused");
    inputs(&dir);
    let input = dir.path("in48.wav");
    let sheet = Sheet::open(&input);
    let out = dir.path("out.opus");
    assert_eq!(
        sheet.change(&json!([{"output": out}, {"sample_rate": 44100}])),
        CF_OK
    );
    let state = sheet.state();
    assert_eq!(state["can_queue"], false);
    let problems = state["problems"].as_array().unwrap();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0]["kind"], "source");
    // The engine's refusal names only flags, which the sheet has none of.
    assert_eq!(
        problems[0]["message"],
        "Crossfade cannot save this combination of choices."
    );
    // The command line refuses the same choice when it runs.
    let args = ["convert", &input, "-o", &out, "--sample-rate", "44100"];
    let outcome = run_command(args, &mut FsIo::new(), &CancelToken::new(), &mut |_| {}).unwrap();
    assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");

    // A value the filter rules refuse sits under its row, as the flag is refused.
    let sheet = Sheet::open(&input);
    let changes = json!([{"output": dir.path("t.wav")}, {"trim_start": "30"}, {"trim_end": "10"}]);
    assert_eq!(sheet.change(&changes), CF_OK);
    let state = sheet.state();
    assert_eq!(state["can_queue"], false);
    assert_eq!(state["problems"][0]["kind"], "adjust");
    assert_eq!(state["problems"][0]["row"], "trim");
    assert_eq!(
        state["adjust"]["problems"]["trim"],
        state["problems"][0]["message"]
    );
    let args = [
        "convert", &input, "-o", "t.wav", "--start", "30s", "--end", "10s",
    ];
    assert!(request_from_args(args).is_err());
}

#[test]
fn the_state_offers_what_the_sheet_offers() {
    let dir = Dir::new("sheet-offers");
    inputs(&dir);
    let sheet = Sheet::open(&dir.path("in48.wav"));
    let state = sheet.state();
    let containers: Vec<&str> = state["container"]["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"].as_str().unwrap())
        .collect();
    assert_eq!(containers, ["wav", "flac", "aiff", "oga", "m4a", "opus"]);
    assert_eq!(state["compression_level"]["visible"], false);
    assert_eq!(state["bitrate"]["visible"], false);
    assert_eq!(state["format"]["visible"], true);
    assert_eq!(state["copy"]["available"], true);
    assert_eq!(state["output"], dir.path("in48-copy.wav"));

    assert_eq!(sheet.change(&json!({"container": "opus"})), CF_OK);
    let state = sheet.state();
    assert_eq!(state["format"]["visible"], false);
    assert_eq!(state["bitrate"]["visible"], true);
    assert_eq!(
        state["bitrate"]["options"][0]["label"],
        "Default (128 kb/s)"
    );
    assert_eq!(state["sample_rate"]["value"], 48000);
    assert_eq!(
        state["sample_rate"]["options"],
        json!([{"value": 48000, "label": "48 kHz"}])
    );
    assert_eq!(state["output"], dir.path("in48.opus"));
}

#[test]
fn an_unreadable_file_opens_no_sheet() {
    let dir = Dir::new("sheet-unreadable");
    let junk = dir.path("junk.wav");
    std::fs::write(&junk, b"nothing").unwrap();
    let path = c(&junk);
    let mut handle = std::ptr::null_mut();
    let status = unsafe { cf_sheet_new(path.as_ptr(), &mut handle) };
    assert_eq!(status, CF_ERR_REFUSED);
    assert!(handle.is_null());
    let facts = json_out(|out| unsafe { cf_inspect(path.as_ptr(), out) });
    assert_eq!(last_error(), None);
    assert!(facts["problem"].is_string());
}

#[test]
fn bad_changes_are_refused_whole_and_leave_the_sheet_as_it_was() {
    let dir = Dir::new("sheet-bad");
    inputs(&dir);
    let sheet = Sheet::open(&dir.path("in48.wav"));
    let before = sheet.state();
    for bad in [
        json!({"colour": "red"}),
        json!({"container": "mp3"}),
        json!({"compression_level": "eight"}),
        json!([{"container": "flac"}, {"format": 16}]),
        json!([{"container": "flac", "output": 3}]),
        json!({"channels": "nine speakers"}),
        json!({"level_mode": "loud"}),
        json!(42),
    ] {
        assert_eq!(sheet.change(&bad), CF_ERR_INVALID_JSON, "{bad}");
        assert!(last_error().is_some());
        assert_eq!(sheet.state(), before, "{bad}");
    }
    let malformed = c("{\"container\": ");
    assert_eq!(
        unsafe { cf_sheet_apply(sheet.0, malformed.as_ptr()) },
        CF_ERR_INVALID_JSON
    );
    let not_utf8 = [b'{', 0xff, b'}', 0];
    assert_eq!(
        unsafe { cf_sheet_apply(sheet.0, not_utf8.as_ptr().cast()) },
        CF_ERR_INVALID_UTF8
    );
    assert_eq!(sheet.state(), before);
}

#[test]
fn null_handles_are_errors_and_destroying_null_does_nothing() {
    let changes = c("{}");
    assert_eq!(
        unsafe { cf_sheet_apply(std::ptr::null_mut(), changes.as_ptr()) },
        CF_ERR_NULL_ARGUMENT
    );
    let mut out = std::ptr::null_mut();
    assert_eq!(
        unsafe { cf_sheet_state(std::ptr::null(), &mut out) },
        CF_ERR_NULL_ARGUMENT
    );
    unsafe { cf_sheet_destroy(std::ptr::null_mut()) };
    let mut handle = std::ptr::null_mut();
    assert_eq!(
        unsafe { cf_sheet_new(std::ptr::null(), &mut handle) },
        CF_ERR_NULL_ARGUMENT
    );
}
