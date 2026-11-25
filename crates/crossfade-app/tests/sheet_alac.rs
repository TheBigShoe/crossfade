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
        "Crossfade can save .wav, .flac, .aiff, .aif, .aifc, .oga, .ogg, .opus, .m4a, .mp4 and .m4b files."
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
fn a_preset_that_had_to_change_the_format_still_reads_as_itself() {
    use SampleFormat::*;
    let mut s = sheet("in.wav", F32);
    // The preset chose "as the source"; the container made that 24-bit, and the hint says so.
    s.apply_preset("alac");
    assert_eq!(s.format(), FormatChoice::Format(S24));
    assert_eq!(s.preset().map(|p| p.id), Some("alac"));
    s.apply_preset("flac");
    assert_eq!(s.preset().map(|p| p.id), Some("flac"));
    // A format the person picks themselves is theirs: no preset asked for it.
    s.apply_preset("alac");
    s.set_format(FormatChoice::Format(S16));
    assert!(s.preset().is_none());
    // A preset asking for exactly what is written still reads as that preset.
    s.apply_preset("flac-24");
    assert_eq!(s.preset().map(|p| p.id), Some("flac-24"));
}

#[test]
fn the_request_is_the_cli_s_for_an_m4a_convert() {
    let mut s = sheet("in.wav", SampleFormat::S24);
    s.apply_preset("alac");
    let line = crossfade_app::command::command_line(&s.build(&nothing_exists).unwrap()).unwrap();
    assert_eq!(line, "crossfade convert in.wav -o in.m4a");
    s.set_format(FormatChoice::Format(SampleFormat::S16));
    let line = crossfade_app::command::command_line(&s.build(&nothing_exists).unwrap()).unwrap();
    assert!(
        line.ends_with("-o in-s16.m4a --sample-format s16"),
        "{line}"
    );
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

/// A silent 16-bit WAVE_FORMAT_EXTENSIBLE file with `channels` channels and `mask`.
fn labelled_io(path: &str, channels: u16, mask: u32) -> MemoryIo {
    let data = vec![0u8; 480 * usize::from(channels) * 2];
    let mut b = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&0xFFFEu16.to_le_bytes());
    b.extend_from_slice(&channels.to_le_bytes());
    b.extend_from_slice(&48_000u32.to_le_bytes());
    b.extend_from_slice(&(48_000 * u32::from(channels) * 2).to_le_bytes());
    b.extend_from_slice(&(channels * 2).to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(&22u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(&mask.to_le_bytes());
    b.extend_from_slice(&[
        1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71,
    ]);
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    let riff = (b.len() - 8) as u32;
    b[4..8].copy_from_slice(&riff.to_le_bytes());
    let mut io = MemoryIo::new();
    io.insert(path, b);
    io
}

#[test]
fn a_layout_alac_cannot_store_is_a_problem_in_plain_words_that_name_only_what_works() {
    // Quad is not ALAC's 4.0, and a matrix is not something the sheet has: the problem alone.
    let mut io = labelled_io("quad.wav", 4, 0x33);
    let mut s = ConvertSheet::for_input(&entry_of(&mut io, "quad.wav"));
    s.set_container(Container::Mp4);
    assert_eq!(
        s.problems(&nothing_exists),
        [Problem::Source(
            "Apple Lossless stores 4 channels in one layout only (4.0) and this stream is quad."
                .into()
        )]
    );
}

#[test]
fn back_surrounds_and_unlabelled_channels_are_no_problem_and_the_sheet_shows_the_note() {
    let mut io = labelled_io("back.wav", 6, 0x3F);
    let mut s = ConvertSheet::for_input(&entry_of(&mut io, "back.wav"));
    s.set_container(Container::Mp4);
    assert!(s.problems(&nothing_exists).is_empty());
    assert_eq!(
        s.notes(),
        [
            "5.1 with back surrounds written as 5.1 with side surrounds: Apple Lossless names \
             its surround pair as side speakers"
        ]
    );
    assert!(s.build(&nothing_exists).is_ok());
    let layout = ChannelLayout::unpositioned(4).unwrap();
    let mut io = wav_io("four.wav", SampleFormat::S16, layout);
    let mut s = ConvertSheet::for_input(&entry_of(&mut io, "four.wav"));
    s.set_container(Container::Mp4);
    assert!(s.problems(&nothing_exists).is_empty());
    assert_eq!(
        s.notes(),
        ["4 unlabelled channels taken as 4.0: Apple Lossless's layout for 4 channels"]
    );
    // ALAC's own layout needs no note.
    let mut io = labelled_io("own.wav", 6, 0x60F);
    let mut s = ConvertSheet::for_input(&entry_of(&mut io, "own.wav"));
    s.set_container(Container::Mp4);
    assert!(s.notes().is_empty());
}

#[test]
fn every_writable_extension_maps_to_its_own_container() {
    use crossfade_graph::writable_extensions;
    let mut s = sheet("in.wav", SampleFormat::S16);
    for ext in writable_extensions() {
        let want = match *ext {
            "wav" => Container::Wav,
            "flac" => Container::Flac,
            "aiff" | "aif" | "aifc" => Container::Aiff,
            "oga" | "ogg" => Container::OggFlac,
            "m4a" | "mp4" | "m4b" => Container::Mp4,
            "opus" => Container::Opus,
            other => panic!("{other} has no container here: add it to this test and the sheet"),
        };
        s.set_container(Container::Wav);
        s.set_output(format!("x.{ext}"));
        assert_eq!(s.container(), want, "{ext}");
        assert!(s.problems(&nothing_exists).is_empty(), "{ext}");
    }
}

#[test]
fn the_bad_extension_message_lists_every_writable_extension() {
    let message = Problem::Extension.message();
    for ext in crossfade_graph::writable_extensions() {
        assert!(
            message.contains(&format!(".{ext}")),
            "{ext} missing from {message}"
        );
    }
}

#[test]
fn an_m4a_job_plans_alac_in_mp4() {
    let mut s = sheet("in.wav", SampleFormat::S16);
    s.apply_preset("alac");
    let request = s.build(&nothing_exists).unwrap();
    let out = &request.job.outputs[0];
    let mut io = wav_io("in.wav", SampleFormat::S16, ChannelLayout::stereo());
    let stream = entry_of(&mut io, "in.wav").stream().unwrap().clone();
    let plan = crossfade_graph::validate(&stream, &out.path, &out.audio).unwrap();
    assert_eq!(plan.output_format, crossfade_format::FormatId::Mp4);
    assert_eq!(plan.output.codec, crossfade_core::CodecId::Alac);
}
