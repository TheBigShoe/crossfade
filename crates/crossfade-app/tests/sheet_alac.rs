//! The convert sheet writing Apple Lossless: a container with no level, whole-number formats,
//! and the engine's refusals in plain words.

mod common;

use common::{sine_wav, spec};
use crossfade_app::inputs::{InputEntry, Inputs, probe_path};
use crossfade_app::sheet::{Container, ConvertSheet, FormatChoice, PRESETS, Problem};
use crossfade_core::{ChannelLayout, Limits, SampleFormat};
use crossfade_graph::{AudioSettings, CancelToken, Job, JobInput, JobOutput, run};
use crossfade_testkit::memory::MemoryIo;

fn entry_of(io: &mut MemoryIo, path: &str) -> InputEntry {
    let mut inputs = Inputs::new();
    inputs.add(path, probe_path(path, io, &Limits::default()));
    inputs.entries()[0].clone()
}

fn wav_io(path: &str, format: SampleFormat, layout: ChannelLayout) -> MemoryIo {
    let (_, wav) = sine_wav(&spec(48_000, format, layout), 200);
    let mut io = MemoryIo::new();
    io.insert(path, wav);
    io
}

fn sheet(path: &str, format: SampleFormat) -> ConvertSheet {
    let mut io = wav_io(path, format, ChannelLayout::stereo());
    ConvertSheet::for_input(&entry_of(&mut io, path))
}

fn nothing_exists(_: &str) -> bool {
    false
}

#[test]
fn apple_lossless_is_a_container_and_a_preset() {
    assert_eq!(Container::Mp4.label(), "Apple Lossless");
    assert_eq!(Container::Mp4.extension(), "m4a");
    assert!(Container::ALL.contains(&Container::Mp4));
    let preset = PRESETS.iter().find(|p| p.id == "alac").unwrap();
    assert_eq!(preset.label, "Apple Lossless");
    let mut s = sheet("song.wav", SampleFormat::S16);
    s.apply_preset("alac");
    assert_eq!(
        (s.container(), s.format()),
        (Container::Mp4, FormatChoice::SameAsSource)
    );
    assert_eq!(s.output(), "song.m4a");
    assert_eq!(s.preset().unwrap().id, "alac");
}

#[test]
fn typing_an_m4a_name_switches_to_it() {
    let mut s = sheet("in.wav", SampleFormat::S16);
    for typed in ["a.m4a", "a.MP4", "a.m4b"] {
        s.set_container(Container::Flac);
        s.set_output(typed.into());
        assert_eq!(s.container(), Container::Mp4, "{typed}");
        assert!(s.problems(&nothing_exists).is_empty(), "{typed}");
    }
    assert_eq!(
        Problem::Extension.message(),
        "Crossfade can save .wav, .flac, .oga, .ogg, .aiff and .m4a files."
    );
}

#[test]
fn there_is_no_compression_level_for_apple_lossless() {
    let mut s = sheet("in.wav", SampleFormat::S16);
    s.set_container(Container::Flac);
    s.set_level(Some(8));
    s.set_container(Container::Mp4);
    assert!(!s.level_visible() && !s.level_enabled());
    assert_eq!(s.level(), None);
    s.set_level(Some(3));
    assert_eq!(s.level(), None);
}

#[test]
fn only_whole_number_formats_are_offered_and_a_float_source_says_so() {
    use SampleFormat::*;
    let mut s = sheet("in.wav", F32);
    s.set_container(Container::Mp4);
    // The float source cannot stay as it is, so "same" is not offered either.
    assert_eq!(
        s.format_options(),
        [S16, S24, S32].map(FormatChoice::Format).to_vec()
    );
    assert_eq!(s.format(), FormatChoice::Format(S24));
    assert_eq!(
        s.hint().as_deref(),
        Some("Apple Lossless stores whole-number samples, so this will be saved as 24-bit.")
    );
}

#[test]
fn the_request_is_the_cli_s_for_an_m4a_convert() {
    let mut s = sheet("in.wav", SampleFormat::S24);
    s.apply_preset("alac");
    let line = crossfade_app::command::command_line(&s.build(&nothing_exists).unwrap()).unwrap();
    assert_eq!(line, "crossfade convert in.wav -o in.m4a");
    s.set_format(FormatChoice::Format(SampleFormat::S16));
    let line = crossfade_app::command::command_line(&s.build(&nothing_exists).unwrap()).unwrap();
    assert!(line.ends_with("-o in-s16.m4a --sample-format s16"), "{line}");
}

#[test]
fn an_m4a_source_can_be_copied_and_a_wav_cannot() {
    let mut io = wav_io("a.wav", SampleFormat::S16, ChannelLayout::stereo());
    let job = Job {
        version: Job::VERSION,
        inputs: vec![JobInput {
            path: "a.wav".into(),
        }],
        outputs: vec![JobOutput {
            path: "a.m4a".into(),
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
    let mut s = ConvertSheet::for_input(&entry_of(&mut io, "a.m4a"));
    s.set_container(Container::Mp4);
    assert!(s.copy_available());
    s.set_copy(true);
    assert!(s.copy());
    let mut s = sheet("a.wav", SampleFormat::S16);
    s.set_container(Container::Mp4);
    assert!(!s.copy_available());
}

#[test]
fn a_layout_alac_cannot_store_is_a_problem_in_plain_words() {
    let layout = ChannelLayout::unpositioned(4).unwrap();
    let mut io = wav_io("four.wav", SampleFormat::S16, layout);
    let mut s = ConvertSheet::for_input(&entry_of(&mut io, "four.wav"));
    s.set_container(Container::Mp4);
    assert_eq!(
        s.problems(&nothing_exists),
        [Problem::Source(
            "Apple Lossless stores 4 channels in one layout only (4.0) and this stream is 4 \
             unlabelled channels."
                .into()
        )]
    );
}
