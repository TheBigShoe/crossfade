mod common;

use common::{sine_wav, spec};
use crossfade_app::inputs::{InputEntry, Inputs, probe_path};
use crossfade_app::sheet::{Container, ConvertSheet, FormatChoice, PRESETS, Problem};
use crossfade_core::{ChannelLayout, Limits, SampleFormat};
use crossfade_graph::{AudioSettings, CancelToken, Job, JobInput, JobOutput, run};
use crossfade_testkit::memory::MemoryIo;

fn p(parts: &[&str]) -> String {
    let mut path = std::path::PathBuf::new();
    path.extend(parts);
    path.to_string_lossy().into_owned()
}

/// A probed WAV of `rate`/`format`, stereo, at `path`.
fn wav_io(path: &str, rate: u32, format: SampleFormat) -> MemoryIo {
    let (_, wav) = sine_wav(&spec(rate, format, ChannelLayout::stereo()), 200);
    let mut io = MemoryIo::new();
    io.insert(path, wav);
    io
}

fn entry_in(io: &mut MemoryIo, path: &str) -> InputEntry {
    let mut inputs = Inputs::new();
    inputs.add(path, probe_path(path, io, &Limits::default()));
    inputs.entries()[0].clone()
}

fn entry(path: &str) -> InputEntry {
    let mut io = wav_io(path, 48_000, SampleFormat::S16);
    entry_in(&mut io, path)
}

fn wav_entry(path: &str, rate: u32, format: SampleFormat) -> InputEntry {
    entry_in(&mut wav_io(path, rate, format), path)
}

/// A probed WAV with `channels` unpositioned channels.
fn wide_entry(path: &str, channels: u16) -> InputEntry {
    let layout = ChannelLayout::unpositioned(channels).unwrap();
    let (_, wav) = sine_wav(&spec(8000, SampleFormat::S16, layout), 200);
    let mut io = MemoryIo::new();
    io.insert(path, wav);
    entry_in(&mut io, path)
}

/// `path` converted by the engine to `to` (a `.flac` or `.aiff` name), then probed.
fn converted_entry(path: &str, to: &str) -> InputEntry {
    let mut io = wav_io(path, 48_000, SampleFormat::S16);
    let job = Job {
        version: Job::VERSION,
        inputs: vec![JobInput { path: path.into() }],
        outputs: vec![JobOutput {
            path: to.into(),
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
    entry_in(&mut io, to)
}

fn sheet(path: &str) -> ConvertSheet {
    ConvertSheet::for_input(&entry(path))
}

fn sheet_of(entry: &InputEntry) -> ConvertSheet {
    ConvertSheet::for_input(entry)
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
            ("float", "32-bit float"),
            ("flac", "FLAC"),
            ("flac-cd", "FLAC 16-bit"),
            ("flac-24", "FLAC 24-bit"),
            ("aiff", "AIFF"),
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
fn flac_and_aiff_presets_set_the_container_and_format() {
    let mut s = sheet("a.wav");
    for (id, container, format) in [
        ("flac", Container::Flac, FormatChoice::SameAsSource),
        (
            "flac-cd",
            Container::Flac,
            FormatChoice::Format(SampleFormat::S16),
        ),
        (
            "flac-24",
            Container::Flac,
            FormatChoice::Format(SampleFormat::S24),
        ),
        ("aiff", Container::Aiff, FormatChoice::SameAsSource),
        ("same", Container::Wav, FormatChoice::SameAsSource),
    ] {
        s.apply_preset(id);
        assert_eq!((s.container(), s.format()), (container, format), "{id}");
        assert_eq!(s.preset().unwrap().id, id);
    }
    // A level makes the combination custom.
    s.apply_preset("flac");
    s.set_level(Some(8));
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
    assert_eq!(s.problems(&nothing_exists), [Problem::Extension]);
    s.set_output("b.WAV".into());
    assert!(s.problems(&nothing_exists).is_empty());
    s.set_output("b".into());
    assert_eq!(s.problems(&nothing_exists), [Problem::Extension]);
    for ogg in ["b.ogg", "b.OGA"] {
        s.set_output(ogg.into());
        assert!(s.problems(&nothing_exists).is_empty(), "{ogg}");
    }
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
        Problem::Extension,
        Problem::Source("x".into()),
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
                    sample_format: Some(SampleFormat::S24),
                    ..AudioSettings::default()
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

fn texts(s: &ConvertSheet) -> Vec<String> {
    let mut all: Vec<String> = s
        .problems(&nothing_exists)
        .iter()
        .map(Problem::message)
        .collect();
    all.extend(s.hint());
    all
}

fn assert_plain(s: &ConvertSheet) {
    for text in texts(s) {
        assert!(!text.contains("--"), "{text}");
    }
}

#[test]
fn the_container_always_matches_the_output_extension() {
    let mut s = sheet(&p(&["music", "song.wav"]));
    assert_eq!(s.container(), Container::Wav);
    s.set_container(Container::Flac);
    assert_eq!(s.output(), p(&["music", "song.flac"]));
    s.set_container(Container::Aiff);
    assert_eq!(s.output(), p(&["music", "song.aiff"]));
    s.set_container(Container::Wav);
    assert_eq!(s.output(), p(&["music", "song-copy.wav"]));
    assert_eq!(
        [Container::Wav, Container::Flac, Container::Aiff].map(Container::label),
        ["WAV", "FLAC", "AIFF"]
    );
}

#[test]
fn choosing_a_container_rewrites_only_the_extension_of_a_typed_path() {
    let mut s = sheet("in.wav");
    s.set_output(p(&["my folder", "take 1.v2.wav"]));
    s.set_container(Container::Flac);
    assert_eq!(s.output(), p(&["my folder", "take 1.v2.flac"]));
    s.set_output(p(&["x", "bare"]));
    s.set_container(Container::Aiff);
    assert_eq!(s.output(), p(&["x", "bare.aiff"]));
    s.set_output(String::new());
    s.set_container(Container::Wav);
    assert_eq!(s.output(), "");
    assert_eq!(s.container(), Container::Wav);
}

#[test]
fn typing_a_writable_extension_switches_the_container() {
    let mut s = sheet("in.wav");
    for (typed, container) in [
        ("a.flac", Container::Flac),
        ("a.AIFF", Container::Aiff),
        ("a.aif", Container::Aiff),
        ("a.aifc", Container::Aiff),
        ("a.wav", Container::Wav),
        ("a.oga", Container::OggFlac),
        ("a.OGG", Container::OggFlac),
    ] {
        s.set_output(typed.into());
        assert_eq!(s.container(), container, "{typed}");
    }
    // Not writable: the container stays and the path is the problem.
    s.set_output("a.flac".into());
    s.set_output("a.mp3".into());
    assert_eq!(s.container(), Container::Flac);
    assert_eq!(s.problems(&nothing_exists), [Problem::Extension]);
    assert_eq!(
        Problem::Extension.message(),
        "Crossfade can save .wav, .flac, .oga, .ogg and .aiff files."
    );
}

#[test]
fn ogg_flac_is_a_container_with_flac_levels_and_flac_rules() {
    let mut s = sheet(&p(&["music", "song.wav"]));
    s.set_container(Container::OggFlac);
    assert_eq!(s.output(), p(&["music", "song.oga"]));
    assert_eq!(Container::OggFlac.label(), "Ogg FLAC");
    assert!(Container::ALL.contains(&Container::OggFlac));
    assert!(s.level_visible() && s.level_enabled());
    s.set_level(Some(3));
    assert_eq!(s.level(), Some(3));
    let request = s.build(&nothing_exists).unwrap();
    let line = crossfade_app::command::command_line(&request).unwrap();
    assert!(line.contains("--compression-level 3"), "{line}");
    assert!(line.contains("song.oga"), "{line}");
    // Whole numbers only, like FLAC.
    assert!(
        !s.format_options()
            .contains(&FormatChoice::Format(SampleFormat::F32))
    );
    s.set_format(FormatChoice::Format(SampleFormat::F32));
    assert_eq!(s.format(), FormatChoice::Format(SampleFormat::S24));
    assert_eq!(
        s.hint().as_deref(),
        Some("FLAC stores whole-number samples, so this will be saved as 24-bit.")
    );
    s.set_container(Container::Wav);
    assert_eq!(s.level(), None);
}

#[test]
fn a_flac_source_can_be_copied_into_ogg_and_a_wav_cannot() {
    let flac = converted_entry("a.wav", "a.flac");
    let mut s = ConvertSheet::for_input(&flac);
    s.set_container(Container::OggFlac);
    assert!(s.copy_available());
    s.set_copy(true);
    assert!(s.copy() && !s.level_enabled());
    let wav = entry("a.wav");
    let mut s = ConvertSheet::for_input(&wav);
    s.set_container(Container::OggFlac);
    assert!(!s.copy_available());
}

#[test]
fn the_default_name_depends_on_the_container_and_the_format() {
    let dir = |name: &str| p(&["music", name]);
    let mut s = sheet(&dir("song.wav"));
    s.set_container(Container::Flac);
    assert_eq!(s.output(), dir("song.flac"));
    s.set_format(FormatChoice::Format(SampleFormat::S16));
    assert_eq!(s.output(), dir("song-s16.flac"));
    s.set_container(Container::Wav);
    assert_eq!(s.output(), dir("song-s16.wav"));

    // The same container as the input never names the input itself.
    let flac = converted_entry(&dir("song.flac"), &dir("song.flac"));
    let mut s = sheet_of(&flac);
    s.set_container(Container::Flac);
    assert_eq!(s.output(), dir("song-copy.flac"));
    s.set_container(Container::Wav);
    assert_eq!(s.output(), dir("song.wav"));

    // After a typed path only the extension follows.
    s.set_output(dir("mine.wav"));
    s.set_container(Container::Aiff);
    s.set_format(FormatChoice::Format(SampleFormat::S24));
    assert_eq!(s.output(), dir("mine.aiff"));
}

#[test]
fn format_options_are_what_the_engine_accepts_for_the_container() {
    use SampleFormat::*;
    let all = |formats: &[SampleFormat]| {
        let mut v = vec![FormatChoice::SameAsSource];
        v.extend(formats.iter().map(|f| FormatChoice::Format(*f)));
        v
    };
    let mut s = sheet("a.wav");
    assert_eq!(s.format_options(), all(&[U8, S16, S24, S32, F32, F64]));
    s.set_container(Container::Aiff);
    assert_eq!(s.format_options(), all(&[U8, S16, S24, S32, F32, F64]));
    s.set_container(Container::Flac);
    assert_eq!(s.format_options(), all(&[U8, S16, S24, S32]));

    let mut s = sheet_of(&wav_entry("f.wav", 48_000, F32));
    s.set_container(Container::Flac);
    assert_eq!(
        s.format_options(),
        [
            FormatChoice::Format(U8),
            FormatChoice::Format(S16),
            FormatChoice::Format(S24),
            FormatChoice::Format(S32)
        ]
    );
}

#[test]
fn a_float_source_into_flac_is_saved_as_24_bit_with_a_hint() {
    let mut s = sheet_of(&wav_entry(&p(&["m", "f.wav"]), 48_000, SampleFormat::F32));
    assert_eq!(s.hint(), None);
    s.set_container(Container::Flac);
    assert_eq!(s.format(), FormatChoice::Format(SampleFormat::S24));
    assert_eq!(
        s.hint().as_deref(),
        Some("FLAC stores whole-number samples, so this will be saved as 24-bit.")
    );
    assert_eq!(s.output(), p(&["m", "f-s24.flac"]));
    let request = s.build(&nothing_exists).unwrap();
    assert_eq!(
        request.job.outputs[0].audio.sample_format,
        Some(SampleFormat::S24)
    );
    assert_eq!(request.job.outputs[0].audio.codec, None);
    assert_plain(&s);

    // Going back restores what the user had asked for.
    s.set_container(Container::Wav);
    assert_eq!(s.format(), FormatChoice::SameAsSource);
    assert_eq!(s.hint(), None);

    // An explicit choice of a valid format needs no hint.
    s.set_container(Container::Flac);
    s.set_format(FormatChoice::Format(SampleFormat::S16));
    assert_eq!(s.hint(), None);

    let s32 = wav_entry("w.wav", 48_000, SampleFormat::S32);
    let mut s = sheet_of(&s32);
    s.apply_preset("flac");
    // FLAC holds 32-bit integers, so a 32-bit source keeps its format.
    assert_eq!(s.format(), FormatChoice::SameAsSource);
    assert_eq!(s.hint(), None);

    // A float source may be saved as 32-bit whole numbers by choice.
    let mut s = sheet_of(&wav_entry("f.wav", 48_000, SampleFormat::F32));
    s.set_container(Container::Flac);
    s.set_format(FormatChoice::Format(SampleFormat::S32));
    assert_eq!(s.format(), FormatChoice::Format(SampleFormat::S32));
    assert_eq!(s.hint(), None);
}

#[test]
fn a_192_khz_source_goes_into_flac_unchanged() {
    let mut s = sheet_of(&wav_entry("hi.wav", 192_000, SampleFormat::S24));
    s.set_container(Container::Flac);
    assert!(s.problems(&nothing_exists).is_empty());
    assert_eq!(s.format(), FormatChoice::SameAsSource);
    assert!(s.build(&nothing_exists).is_ok());
}

#[test]
fn a_source_with_more_than_8_channels_cannot_go_into_flac() {
    let mut s = sheet_of(&wide_entry("wide.wav", 9));
    assert!(s.problems(&nothing_exists).is_empty());
    s.set_container(Container::Flac);
    assert!(s.format_options().is_empty());
    let want = "FLAC can store up to 8 channels; this file has 9.";
    assert_eq!(
        s.problems(&nothing_exists),
        [Problem::Source(want.to_owned())]
    );
    assert!(s.build(&nothing_exists).is_err());
    assert_plain(&s);
    s.set_container(Container::Aiff);
    assert!(s.problems(&nothing_exists).is_empty());
}

#[test]
fn the_level_belongs_to_flac_alone() {
    let mut s = sheet("a.wav");
    assert!(!s.level_visible());
    s.set_level(Some(3));
    assert_eq!(s.level(), None);

    s.set_container(Container::Flac);
    assert!(s.level_visible());
    s.set_level(Some(9));
    assert_eq!(s.level(), None);
    s.set_level(Some(8));
    let audio = s.build(&nothing_exists).unwrap().job.outputs[0]
        .audio
        .clone();
    assert_eq!(audio.compression_level, Some(8));
    assert_eq!(audio.codec, None);
    s.set_level(Some(0));
    assert_eq!(s.level(), Some(0));
    s.set_level(None);
    assert_eq!(s.level(), None);

    s.set_level(Some(4));
    s.set_container(Container::Aiff);
    assert_eq!(s.level(), None);
    s.set_container(Container::Flac);
    assert_eq!(s.level(), None);

    // Typing a path of another container clears it as well.
    s.set_level(Some(2));
    s.set_output("x.wav".into());
    assert_eq!(s.level(), None);
}

#[test]
fn copy_is_offered_only_where_the_engine_can_copy() {
    let flac = converted_entry("a.wav", "a.flac");
    let aiff = converted_entry("a.wav", "a.aiff");
    let wav = entry("a.wav");
    for (source, container, offered) in [
        (&flac, Container::Flac, true),
        (&flac, Container::Wav, false),
        (&flac, Container::Aiff, false),
        (&wav, Container::Wav, true),
        (&wav, Container::Flac, false),
        (&wav, Container::Aiff, false),
        (&aiff, Container::Aiff, true),
        (&aiff, Container::Wav, false),
    ] {
        let mut s = sheet_of(source);
        s.set_container(container);
        assert_eq!(s.copy_available(), offered, "{container:?}");
        s.set_copy(true);
        assert_eq!(s.copy(), offered);
    }
}

#[test]
fn copy_replaces_the_format_and_level_in_the_request() {
    let mut s = sheet_of(&converted_entry("a.wav", "a.flac"));
    s.apply_preset("flac-cd");
    s.set_level(Some(6));
    assert!(s.format_enabled() && s.level_enabled());
    s.set_copy(true);
    assert!(!s.format_enabled() && !s.level_enabled());
    assert!(s.preset().is_none());
    let audio = s.build(&nothing_exists).unwrap().job.outputs[0]
        .audio
        .clone();
    assert_eq!(
        audio,
        AudioSettings {
            sample_format: None,
            codec: Some("copy".into()),
            compression_level: None,
            ..AudioSettings::default()
        }
    );
    // Copying a FLAC to a FLAC next to it.
    assert_eq!(s.output(), "a-copy.flac");
    s.set_copy(false);
    assert_eq!(s.format(), FormatChoice::Format(SampleFormat::S16));
    assert_eq!(s.level(), Some(6));

    // Leaving the container drops the copy; coming back does not bring it back unasked.
    s.set_copy(true);
    s.set_container(Container::Wav);
    assert!(!s.copy());
    assert_eq!(
        s.build(&nothing_exists).unwrap().job.outputs[0].audio.codec,
        None
    );
}

#[test]
fn engine_wording_loses_its_flags() {
    use crossfade_app::sheet::plain;
    assert_eq!(
        plain("flac cannot store floating-point samples; choose --sample-format s16, s24 or s32"),
        "Flac cannot store floating-point samples."
    );
    assert_eq!(
        plain("--codec copy cannot change the sample format"),
        "Crossfade cannot save this combination of choices."
    );
    assert_eq!(plain("no decoder for foo."), "No decoder for foo.");
}

#[test]
fn a_source_flac_cannot_take_leaves_the_audio_choices_disabled() {
    let mut s = sheet_of(&wide_entry("wide.wav", 9));
    s.set_container(Container::Flac);
    assert!(s.level_visible());
    assert!(!s.level_enabled());
    assert!(!s.format_enabled());
    s.set_container(Container::Aiff);
    assert!(s.format_enabled());
    s.set_container(Container::Flac);
    let mut ok = sheet_of(&wav_entry("lo.wav", 48_000, SampleFormat::S16));
    ok.set_container(Container::Flac);
    assert!(ok.level_enabled());
    assert!(ok.format_enabled());
}
