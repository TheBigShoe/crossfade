use crossfade_app::inputs::{InputEntry, InputState, Inputs};
use crossfade_app::sheet::{ConvertSheet, FormatChoice, PRESETS, Problem};
use crossfade_core::SampleFormat;
use crossfade_graph::{AudioSettings, Job, JobInput, JobOutput};

fn p(parts: &[&str]) -> String {
    let mut path = std::path::PathBuf::new();
    path.extend(parts);
    path.to_string_lossy().into_owned()
}

fn entry(path: &str) -> InputEntry {
    let mut inputs = Inputs::new();
    inputs.add(path, InputState::Failed(String::new()));
    inputs.entries()[0].clone()
}

fn sheet(path: &str) -> ConvertSheet {
    ConvertSheet::for_input(&entry(path))
}

fn nothing_exists(_: &str) -> bool {
    false
}

#[test]
fn default_output_sits_beside_the_input() {
    let input = p(&["music", "song.wav"]);
    assert_eq!(sheet(&input).output(), p(&["music", "song-copy.wav"]));
    assert_eq!(sheet("song.wav").output(), "song-copy.wav");
    assert_eq!(sheet("noext").output(), "noext-copy.wav");
}

#[test]
fn the_default_output_follows_the_format_until_the_user_edits_it() {
    let input = p(&["music", "song.wav"]);
    let mut s = sheet(&input);
    s.set_format(FormatChoice::Format(SampleFormat::S24));
    assert_eq!(s.output(), p(&["music", "song-s24.wav"]));
    s.apply_preset("cd");
    assert_eq!(s.output(), p(&["music", "song-s16.wav"]));
    s.apply_preset("same");
    assert_eq!(s.output(), p(&["music", "song-copy.wav"]));

    s.set_output(p(&["elsewhere", "mine.wav"]));
    s.set_format(FormatChoice::Format(SampleFormat::F32));
    assert_eq!(s.output(), p(&["elsewhere", "mine.wav"]));
}

#[test]
fn presets_map_to_formats_and_back() {
    let ids: Vec<_> = PRESETS.iter().map(|p| (p.id, p.label)).collect();
    assert_eq!(
        ids,
        [
            ("same", "Same as source"),
            ("cd", "16-bit (CD)"),
            ("studio", "24-bit"),
            ("float", "32-bit float")
        ]
    );
    let mut s = sheet("a.wav");
    assert_eq!(s.preset().unwrap().id, "same");
    for (id, format) in [
        ("cd", SampleFormat::S16),
        ("studio", SampleFormat::S24),
        ("float", SampleFormat::F32),
    ] {
        s.apply_preset(id);
        assert_eq!(s.format(), FormatChoice::Format(format));
        assert_eq!(s.preset().unwrap().id, id);
    }
    s.set_format(FormatChoice::Format(SampleFormat::U8));
    assert!(s.preset().is_none());
}

#[test]
fn an_unknown_preset_is_ignored() {
    let mut s = sheet("a.wav");
    s.apply_preset("nope");
    assert_eq!(s.format(), FormatChoice::SameAsSource);
}

#[test]
fn a_fresh_sheet_has_no_problems() {
    assert!(sheet("a.wav").problems(&nothing_exists).is_empty());
}

#[test]
fn problems_are_named() {
    let mut s = sheet("a.wav");
    s.set_output(String::new());
    assert_eq!(s.problems(&nothing_exists), [Problem::NoOutput]);
    s.set_output("   ".into());
    assert_eq!(s.problems(&nothing_exists), [Problem::NoOutput]);

    s.set_output("a.wav".into());
    assert_eq!(s.problems(&nothing_exists), [Problem::OutputIsInput]);

    s.set_output("b.mp3".into());
    assert_eq!(s.problems(&nothing_exists), [Problem::NotWav]);
    s.set_output("b.WAV".into());
    assert!(s.problems(&nothing_exists).is_empty());
    s.set_output("b".into());
    assert_eq!(s.problems(&nothing_exists), [Problem::NotWav]);
}

#[test]
fn an_existing_output_is_a_problem_only_without_overwrite() {
    let exists = |path: &str| path == "a-copy.wav";
    let mut s = sheet("a.wav");
    assert_eq!(s.problems(&exists), [Problem::OutputExists]);
    s.set_overwrite(true);
    assert!(s.overwrite());
    assert!(s.problems(&exists).is_empty());
}

#[test]
fn every_problem_has_a_message() {
    for problem in [
        Problem::NoOutput,
        Problem::OutputIsInput,
        Problem::OutputExists,
        Problem::NotWav,
    ] {
        assert!(!problem.message().is_empty());
    }
}

#[test]
fn build_makes_the_same_job_the_cli_would() {
    let mut s = sheet("in.wav");
    s.set_output("out.wav".into());
    s.set_format(FormatChoice::Format(SampleFormat::S24));
    s.set_overwrite(true);
    let request = s.build(&nothing_exists).unwrap();
    assert!(request.overwrite);
    assert_eq!(
        request.job,
        Job {
            version: Job::VERSION,
            inputs: vec![JobInput {
                path: "in.wav".into()
            }],
            outputs: vec![JobOutput {
                path: "out.wav".into(),
                audio: AudioSettings {
                    sample_format: Some(SampleFormat::S24)
                },
            }],
        }
    );
}

#[test]
fn same_as_source_leaves_the_format_unset() {
    let request = sheet("in.wav").build(&nothing_exists).unwrap();
    assert_eq!(request.job.outputs[0].audio.sample_format, None);
    assert!(!request.overwrite);
}

#[test]
fn build_refuses_with_the_problems() {
    let mut s = sheet("in.wav");
    s.set_output("in.wav".into());
    assert_eq!(s.build(&nothing_exists), Err(vec![Problem::OutputIsInput]));
}
