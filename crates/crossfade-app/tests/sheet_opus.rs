//! The convert sheet writing Opus: a container and a preset with a bitrate and no level or
//! format, 48 kHz only, mono or stereo, the engine's refusals in plain words, and the notes
//! the command line prints.

mod common;

use common::{sine_wav, spec};
use crossfade_app::inputs::{InputEntry, Inputs, probe_path};
use crossfade_app::sheet::{Container, ConvertSheet, FormatChoice, PRESETS, Problem};
use crossfade_core::{ChannelLayout, Limits, SampleFormat};
use crossfade_graph::filter::parse_channels;
use crossfade_graph::{AudioSettings, CancelToken, Job, JobInput, JobOutput, run};
use crossfade_testkit::memory::MemoryIo;

fn entry_of(io: &mut MemoryIo, path: &str) -> InputEntry {
    let mut inputs = Inputs::new();
    inputs.add(path, probe_path(path, io, &Limits::default()));
    inputs.entries()[0].clone()
}

fn wav_sheet(rate: u32, layout: ChannelLayout) -> ConvertSheet {
    let (_, wav) = sine_wav(&spec(rate, SampleFormat::S16, layout), 4_800);
    let mut io = MemoryIo::new();
    io.insert("song.wav", wav);
    ConvertSheet::for_input(&entry_of(&mut io, "song.wav"))
}

/// A sheet for an Opus file the engine itself wrote.
/// A sheet for a silent 16-bit WAVE_FORMAT_EXTENSIBLE file with `channels` channels and `mask`.
fn labelled_sheet(channels: u16, mask: u32) -> ConvertSheet {
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
    io.insert("surround.wav", b);
    ConvertSheet::for_input(&entry_of(&mut io, "surround.wav"))
}

fn opus_sheet() -> ConvertSheet {
    let (_, wav) = sine_wav(
        &spec(48_000, SampleFormat::S16, ChannelLayout::stereo()),
        4_800,
    );
    let mut io = MemoryIo::new();
    io.insert("seed.wav", wav);
    let job = Job {
        version: Job::VERSION,
        inputs: vec![JobInput {
            path: "seed.wav".into(),
        }],
        outputs: vec![JobOutput {
            path: "song.opus".into(),
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
    ConvertSheet::for_input(&entry_of(&mut io, "song.opus"))
}

fn nothing_exists(_: &str) -> bool {
    false
}

#[test]
fn opus_is_a_container_and_a_preset() {
    assert_eq!(Container::Opus.label(), "Opus");
    assert_eq!(Container::Opus.extension(), "opus");
    assert!(Container::ALL.contains(&Container::Opus));
    let preset = PRESETS.iter().find(|p| p.id == "opus").unwrap();
    assert_eq!(preset.label, "Opus");
    let mut s = wav_sheet(48_000, ChannelLayout::stereo());
    s.apply_preset("opus");
    assert_eq!(s.container(), Container::Opus);
    assert_eq!(s.output(), "song.opus");
    assert_eq!(s.preset().unwrap().id, "opus");
    s.set_container(Container::Wav);
    s.set_output("a.OPUS".into());
    assert_eq!(s.container(), Container::Opus);
    assert!(s.problems(&nothing_exists).is_empty());
}

#[test]
fn the_bitrate_shows_for_opus_only_and_is_what_the_job_says() {
    let mut s = wav_sheet(48_000, ChannelLayout::stereo());
    assert!(!s.bitrate_visible());
    s.set_bitrate(Some(96_000));
    assert_eq!(s.bitrate(), None);
    s.set_container(Container::Opus);
    assert!(s.bitrate_visible() && s.bitrate_enabled());
    assert_eq!(s.bitrate_label(None), "Default (128 kb/s)");
    assert_eq!(s.bitrate_label(Some(96_000)), "96 kb/s");
    assert!(s.bitrate_options().contains(&None));
    assert!(s.bitrate_options().contains(&Some(64_000)));
    s.set_bitrate(Some(96_000));
    let audio = &s.build(&nothing_exists).unwrap().job.outputs[0].audio;
    assert_eq!(
        (audio.bitrate, audio.codec.as_deref()),
        (Some(96_000), None)
    );
    s.set_container(Container::Flac);
    assert_eq!(s.bitrate(), None);
    assert!(
        s.build(&nothing_exists).unwrap().job.outputs[0]
            .audio
            .bitrate
            .is_none()
    );
}

#[test]
fn mono_defaults_to_96() {
    let mut s = wav_sheet(48_000, ChannelLayout::mono());
    s.set_container(Container::Opus);
    assert_eq!(s.bitrate_label(None), "Default (96 kb/s)");
}

#[test]
fn there_is_no_level_and_no_format_for_opus() {
    let mut s = wav_sheet(48_000, ChannelLayout::stereo());
    s.set_format(FormatChoice::Format(SampleFormat::S16));
    s.set_container(Container::Opus);
    assert!(!s.level_visible());
    assert!(!s.format_visible());
    assert_eq!(s.format(), FormatChoice::SameAsSource);
    assert_eq!(s.format_options(), [FormatChoice::SameAsSource]);
    let r = s.build(&nothing_exists).unwrap();
    assert_eq!(r.job.outputs[0].audio.sample_format, None);
    s.set_container(Container::Wav);
    assert!(s.format_visible());
}

#[test]
fn only_48_khz_is_offered_and_another_rate_is_dropped() {
    let mut s = wav_sheet(44_100, ChannelLayout::stereo());
    s.set_sample_rate(Some(96_000));
    s.set_container(Container::Opus);
    // The row says what is written: 48 kHz, the only choice, and selected.
    assert_eq!(s.rate_options(), [Some(48_000)]);
    assert_eq!(s.sample_rate(), Some(48_000));
    assert_eq!(ConvertSheet::rate_label(s.sample_rate()), "48 kHz");
    assert!(s.problems(&nothing_exists).is_empty());
    // Nothing is asked of the engine: it resamples by itself, as without --sample-rate.
    let audio = &s.build(&nothing_exists).unwrap().job.outputs[0].audio;
    assert_eq!(audio.sample_rate, None);
    // A source already at 48 kHz shows the same row.
    let mut s = wav_sheet(48_000, ChannelLayout::stereo());
    s.set_container(Container::Opus);
    assert_eq!((s.rate_options(), s.sample_rate()), (vec![Some(48_000)], Some(48_000)));
    s.set_container(Container::Wav);
    assert!(s.rate_options().len() > 2);
    assert_eq!(s.sample_rate(), None);
}

#[test]
fn a_surround_source_that_is_not_opuss_is_refused_in_words_that_name_no_flag() {
    // Four channels as 4.0 (front and back centre) are not Vorbis's quad; stereo is a way out.
    let mut s = labelled_sheet(4, 0x107);
    s.set_container(Container::Opus);
    let problems = s.problems(&nothing_exists);
    let [Problem::Source(text)] = problems.as_slice() else {
        panic!("{problems:?}");
    };
    assert_eq!(
        text,
        "Opus stores 4 channels in one layout only (quad) and this stream is 4.0. Choose \
         Mono or Stereo under Channels."
    );
    // Choosing Stereo clears it.
    s.set_channels(Some(parse_channels("stereo").unwrap()));
    assert!(s.problems(&nothing_exists).is_empty());
    // Vorbis's 5.1 is written as it is, with a surround default bitrate and larger choices.
    let mut s = labelled_sheet(6, 0x3F);
    s.set_container(Container::Opus);
    assert!(
        s.problems(&nothing_exists).is_empty(),
        "{:?}",
        s.problems(&nothing_exists)
    );
    assert_eq!(s.bitrate_label(None), "Default (320 kb/s)");
    assert!(s.bitrate_options().contains(&Some(512_000)));
    assert!(!s.bitrate_options().contains(&Some(32_000)));
    s.set_bitrate(Some(384_000));
    assert!(s.build(&nothing_exists).is_ok());
    // More than eight channels.
    let mut s = wav_sheet(48_000, ChannelLayout::unpositioned(9).unwrap());
    s.set_container(Container::Opus);
    let problems = s.problems(&nothing_exists);
    let [Problem::Source(text)] = problems.as_slice() else {
        panic!("{problems:?}");
    };
    assert_eq!(
        text,
        "Opus holds at most 8 channels, got 9. Choose Mono or Stereo under Channels."
    );
}

#[test]
fn side_surrounds_and_unlabelled_channels_are_no_problem_and_the_sheet_shows_the_note() {
    let side = "5.1 with side surrounds written as 5.1 with back surrounds: opus names its \
                surround pair as back speakers";
    let mut s = labelled_sheet(6, 0x60F);
    s.set_container(Container::Opus);
    assert!(s.problems(&nothing_exists).is_empty());
    assert_eq!(s.notes(), [side]);
    // The plan says what is written: Vorbis's layout, with its surround default bitrate.
    assert_eq!(s.bitrate_label(None), "Default (320 kb/s)");
    assert!(s.build(&nothing_exists).is_ok());
    // A plain WAV header carries no layout: six channels are 5.1.
    let mut s = wav_sheet(48_000, ChannelLayout::unpositioned(6).unwrap());
    s.set_container(Container::Opus);
    assert!(s.problems(&nothing_exists).is_empty());
    assert_eq!(
        s.notes(),
        ["6 unlabelled channels taken as 5.1 with back surrounds: opus's layout for 6 channels"]
    );
    // Opus's own layout needs no note.
    let mut s = labelled_sheet(6, 0x3F);
    s.set_container(Container::Opus);
    assert!(s.notes().is_empty());
}

#[test]
fn the_notes_are_the_ones_the_command_line_prints() {
    // A resample Opus makes by itself.
    let mut s = wav_sheet(44_100, ChannelLayout::stereo());
    s.set_container(Container::Opus);
    assert_eq!(
        s.notes(),
        ["opus is always 48 kHz: resampling from 44100 Hz"]
    );
    assert!(s.processing_hint().unwrap().starts_with("48 kHz"));
    // A lossy source written as 16-bit: no processing asked, the note shows all the same.
    let mut s = opus_sheet();
    s.set_container(Container::Wav);
    assert_eq!(
        s.notes(),
        ["opus has no bit depth: writing 16-bit; choose --sample-format to change"]
    );
    s.set_format(FormatChoice::Format(SampleFormat::S24));
    assert!(s.notes().is_empty());
    // Nothing assumed, nothing said.
    let mut s = wav_sheet(48_000, ChannelLayout::stereo());
    s.set_container(Container::Opus);
    assert!(s.notes().is_empty());
}

#[test]
fn the_ogg_container_is_named_for_what_it_holds() {
    let mut s = wav_sheet(48_000, ChannelLayout::stereo());
    s.set_container(Container::OggFlac);
    assert_eq!(s.container_label(Container::OggFlac), "Ogg FLAC");
    assert_eq!(s.container_label(Container::Opus), "Opus");
    // A stream copy of an Opus file into Ogg writes no FLAC.
    let mut s = opus_sheet();
    s.set_container(Container::OggFlac);
    s.set_copy(true);
    assert!(s.copy());
    assert_eq!(s.container_label(Container::OggFlac), "Ogg");
    s.set_container(Container::Opus);
    assert!(s.copy_available());
}

/// A sheet for a FLAC file the engine itself wrote.
fn flac_sheet() -> ConvertSheet {
    let (_, wav) = sine_wav(
        &spec(48_000, SampleFormat::S16, ChannelLayout::stereo()),
        4_800,
    );
    let mut io = MemoryIo::new();
    io.insert("seed.wav", wav);
    let job = Job {
        version: Job::VERSION,
        inputs: vec![JobInput {
            path: "seed.wav".into(),
        }],
        outputs: vec![JobOutput {
            path: "song.flac".into(),
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
    ConvertSheet::for_input(&entry_of(&mut io, "song.flac"))
}

#[test]
fn copy_into_opus_is_offered_for_opus_only() {
    let mut flac = flac_sheet();
    flac.set_container(Container::Flac);
    assert!(flac.copy_available());
    flac.set_container(Container::Opus);
    assert!(!flac.copy_available());
    flac.set_copy(true);
    assert!(!flac.copy());
    let mut opus = opus_sheet();
    opus.set_container(Container::Opus);
    assert!(opus.copy_available());
}
