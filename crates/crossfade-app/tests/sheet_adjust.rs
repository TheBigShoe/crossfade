//! The sheet's processing controls: sample rate, channels and the Adjust section. They are
//! checked against the request the sheet builds (`draft`, so a filter the engine has not
//! built yet changes nothing here) and against the engine's own `describe`.

mod common;

use common::{sine_wav, spec};
use crossfade_app::inputs::{InputEntry, Inputs, probe_path};
use crossfade_app::sheet::{
    AdjustField, AdjustRow, ConvertSheet, FormatChoice, LevelMode, Problem,
};
use crossfade_core::{ChannelLayout, Limits, SampleFormat};
use crossfade_graph::filter::{FilterSpec, parse_channels};
use crossfade_testkit::memory::MemoryIo;

fn entry(rate: u32, layout: ChannelLayout, frames: usize) -> InputEntry {
    let (_, wav) = sine_wav(&spec(rate, SampleFormat::S16, layout), frames);
    let mut io = MemoryIo::new();
    io.insert("in.wav", wav);
    let mut inputs = Inputs::new();
    inputs.add("in.wav", probe_path("in.wav", &mut io, &Limits::default()));
    inputs.entries()[0].clone()
}

/// 48 kHz stereo, 200 frames.
fn sheet() -> ConvertSheet {
    ConvertSheet::for_input(&entry(48_000, ChannelLayout::stereo(), 200))
}

fn nothing_exists(_: &str) -> bool {
    false
}

fn filters(sheet: &ConvertSheet) -> Vec<String> {
    sheet.draft().job.outputs[0]
        .audio
        .filters
        .iter()
        .map(ToString::to_string)
        .collect()
}

fn with(field: AdjustField, text: &str) -> ConvertSheet {
    let mut s = sheet();
    if field == AdjustField::Level {
        s.set_level_mode(LevelMode::Gain);
    }
    s.set_adjust_text(field, text.into());
    s
}

#[test]
fn nothing_set_changes_nothing() {
    let s = sheet();
    let audio = &s.draft().job.outputs[0].audio;
    assert_eq!(audio.sample_rate, None);
    assert_eq!(audio.channels, None);
    assert!(audio.filters.is_empty());
    assert!(!s.adjust_open());
    assert_eq!(s.adjust_summary(), None);
    assert_eq!(s.processing_hint(), None);
    assert!(s.build(&nothing_exists).is_ok());
}

#[test]
fn the_rate_choices_leave_out_the_sources_own_rate() {
    let rates = |s: &ConvertSheet| s.rate_options();
    assert_eq!(
        rates(&sheet()),
        [
            None,
            Some(44_100),
            Some(88_200),
            Some(96_000),
            Some(176_400),
            Some(192_000)
        ]
    );
    let at_44 = ConvertSheet::for_input(&entry(44_100, ChannelLayout::stereo(), 200));
    assert_eq!(
        at_44.rate_options(),
        [
            None,
            Some(48_000),
            Some(88_200),
            Some(96_000),
            Some(176_400),
            Some(192_000)
        ]
    );
    assert_eq!(ConvertSheet::rate_label(None), "Same as source");
    assert_eq!(ConvertSheet::rate_label(Some(44_100)), "44.1 kHz");
    assert_eq!(ConvertSheet::rate_label(Some(48_000)), "48 kHz");
    assert_eq!(ConvertSheet::rate_label(Some(176_400)), "176.4 kHz");
}

#[test]
fn the_channel_choices_leave_out_the_sources_own_layout() {
    let stereo = sheet();
    assert_eq!(
        stereo.channel_options(),
        [None, Some(parse_channels("mono").unwrap())]
    );
    let mono = ConvertSheet::for_input(&entry(48_000, ChannelLayout::mono(), 200));
    assert_eq!(
        mono.channel_options(),
        [None, Some(parse_channels("stereo").unwrap())]
    );
    assert_eq!(ConvertSheet::channel_label(&None), "Same as source");
    let label = |name| ConvertSheet::channel_label(&Some(parse_channels(name).unwrap()));
    assert_eq!(label("mono"), "Mono");
    assert_eq!(label("stereo"), "Stereo");
}

#[test]
fn rate_and_channels_become_settings() {
    let mut s = sheet();
    s.set_sample_rate(Some(44_100));
    s.set_channels(Some(parse_channels("mono").unwrap()));
    let audio = &s.draft().job.outputs[0].audio;
    assert_eq!(audio.sample_rate, Some(44_100));
    assert_eq!(audio.channels, Some(parse_channels("mono").unwrap()));
    assert!(audio.filters.is_empty());
    s.set_sample_rate(None);
    s.set_channels(None);
    assert_eq!(s.draft().job.outputs[0].audio.sample_rate, None);
    assert_eq!(s.draft().job.outputs[0].audio.channels, None);
}

#[test]
fn gain_takes_what_a_person_types() {
    for (typed, filter, value) in [
        ("3", "gain 3dB", "3dB"),
        ("-3", "gain -3dB", "-3dB"),
        ("+3", "gain 3dB", "+3dB"),
        ("-3dB", "gain -3dB", "-3dB"),
        ("-3 dB", "gain -3dB", "-3dB"),
        ("-3db", "gain -3dB", "-3dB"),
        ("\u{2212}3", "gain -3dB", "-3dB"),
        (" -1.5 ", "gain -1.5dB", "-1.5dB"),
    ] {
        let s = with(AdjustField::Level, typed);
        assert_eq!(filters(&s), [filter], "{typed:?}");
        assert_eq!(
            s.adjust_value(AdjustField::Level).as_deref(),
            Some(value),
            "{typed:?}"
        );
        assert_eq!(s.adjust_problem(AdjustRow::Level), None, "{typed:?}");
    }
}

#[test]
fn normalize_takes_what_a_person_types() {
    for typed in ["-16", "-16 LUFS", "-16lufs", "-16LUFS", "\u{2212}16"] {
        let mut s = sheet();
        s.set_level_mode(LevelMode::Normalize);
        s.set_adjust_text(AdjustField::Level, typed.into());
        assert_eq!(
            s.draft().job.outputs[0].audio.filters,
            [FilterSpec::Normalize {
                lufs: -16.0,
                true_peak_dbtp: -1.0
            }],
            "{typed:?}"
        );
    }
}

#[test]
fn each_level_mode_keeps_its_own_value() {
    let mut s = sheet();
    s.set_level_mode(LevelMode::Gain);
    s.set_adjust_text(AdjustField::Level, "-3".into());
    s.set_level_mode(LevelMode::Normalize);
    assert_eq!(s.adjust_text(AdjustField::Level), "");
    assert!(filters(&s).is_empty());
    s.set_adjust_text(AdjustField::Level, "-16".into());
    s.set_level_mode(LevelMode::Gain);
    assert_eq!(s.adjust_text(AdjustField::Level), "-3");
    assert_eq!(filters(&s), ["gain -3dB"]);
    s.set_level_mode(LevelMode::Off);
    assert!(filters(&s).is_empty());
}

#[test]
fn times_take_what_a_person_types() {
    for (typed, seconds) in [
        ("90", 90.0),
        ("90s", 90.0),
        ("1:30", 90.0),
        ("1:30.5", 90.5),
        ("500ms", 0.5),
        ("0:10", 10.0),
        ("1:02:03", 3723.0),
        ("1.5", 1.5),
        (" 12 s ", 12.0),
    ] {
        let s = with(AdjustField::Start, typed);
        assert_eq!(
            s.draft().job.outputs[0].audio.filters,
            [FilterSpec::Trim {
                start_seconds: Some(seconds),
                end_seconds: None,
                duration_seconds: None
            }],
            "{typed:?}"
        );
        assert_eq!(s.adjust_problem(AdjustRow::Trim), None, "{typed:?}");
    }
}

#[test]
fn trim_and_fade_fields_fill_their_own_filters() {
    let mut s = sheet();
    s.set_adjust_text(AdjustField::Start, "10".into());
    s.set_adjust_text(AdjustField::End, "1:30".into());
    s.set_adjust_text(AdjustField::FadeIn, "500ms".into());
    s.set_adjust_text(AdjustField::FadeOut, "3".into());
    assert_eq!(
        filters(&s),
        ["trim start=10s end=90s", "fade in=0.5s out=3s"]
    );
    s.set_adjust_text(AdjustField::Start, "".into());
    s.set_adjust_text(AdjustField::FadeIn, "  ".into());
    assert_eq!(filters(&s), ["trim end=90s", "fade out=3s"]);
}

#[test]
fn filters_are_listed_in_the_order_the_shortcut_flags_give() {
    let mut s = sheet();
    // Set in the opposite order on purpose.
    s.set_level_mode(LevelMode::Gain);
    s.set_adjust_text(AdjustField::Level, "-3".into());
    s.set_adjust_text(AdjustField::FadeOut, "3".into());
    s.set_adjust_text(AdjustField::Start, "1".into());
    assert_eq!(
        filters(&s),
        ["trim start=1s", "fade out=3s", "gain -3dB"]
    );
}

#[test]
fn a_value_that_does_not_parse_blocks_the_sheet_and_names_its_row() {
    for (field, row, typed) in [
        (AdjustField::Level, AdjustRow::Level, "loud"),
        (AdjustField::Level, AdjustRow::Level, "3 s"),
        (AdjustField::Start, AdjustRow::Trim, "soon"),
        (AdjustField::End, AdjustRow::Trim, "1:5"),
        (AdjustField::FadeIn, AdjustRow::Fade, "-2"),
        (AdjustField::FadeOut, AdjustRow::Fade, "2 dB"),
    ] {
        let s = with(field, typed);
        let message = s.adjust_problem(row).unwrap_or_default();
        assert!(!message.is_empty(), "{typed:?}");
        assert!(!message.contains('"'), "{message}");
        assert!(filters(&s).is_empty(), "{typed:?} adds nothing");
        let problems = s.build(&nothing_exists).unwrap_err();
        assert!(
            problems.contains(&Problem::Adjust {
                row,
                message: message.clone()
            }),
            "{typed:?} {problems:?}"
        );
    }
}

#[test]
fn a_value_the_engine_refuses_blocks_the_sheet_with_the_engines_reason() {
    let mut s = sheet();
    s.set_adjust_text(AdjustField::Start, "30".into());
    s.set_adjust_text(AdjustField::End, "10".into());
    let message = s.adjust_problem(AdjustRow::Trim).unwrap();
    assert!(message.to_lowercase().contains("end must be after start"), "{message}");
    assert!(s.build(&nothing_exists).is_err());
    assert_eq!(s.adjust_problem(AdjustRow::Fade), None);
    // Mending it clears the problem.
    s.set_adjust_text(AdjustField::End, "40".into());
    assert_eq!(s.adjust_problem(AdjustRow::Trim), None);
    assert!(s.build(&nothing_exists).is_ok());

    let s = with(AdjustField::Level, "100");
    assert!(s.adjust_problem(AdjustRow::Level).is_some());
    let s = with(AdjustField::FadeIn, "0");
    assert!(s.adjust_problem(AdjustRow::Fade).is_some());
}

#[test]
fn an_empty_value_is_not_a_problem() {
    let mut s = sheet();
    s.set_level_mode(LevelMode::Normalize);
    assert_eq!(s.adjust_problem(AdjustRow::Level), None);
    assert!(s.build(&nothing_exists).is_ok());
}

#[test]
fn the_summary_says_what_is_set_in_one_muted_line() {
    let mut s = sheet();
    s.set_level_mode(LevelMode::Normalize);
    s.set_adjust_text(AdjustField::Level, "-16 LUFS".into());
    assert_eq!(s.adjust_summary().as_deref(), Some("Normalize \u{2212}16 LUFS"));
    s.set_adjust_text(AdjustField::Start, "10".into());
    s.set_adjust_text(AdjustField::End, "1:30".into());
    s.set_adjust_text(AdjustField::FadeOut, "3".into());
    assert_eq!(
        s.adjust_summary().as_deref(),
        Some("Normalize \u{2212}16 LUFS \u{b7} Trim 0:10\u{2013}1:30 \u{b7} Fade out 3 s")
    );
    s.set_level_mode(LevelMode::Gain);
    s.set_adjust_text(AdjustField::Level, "-3".into());
    s.set_adjust_text(AdjustField::Start, "".into());
    s.set_adjust_text(AdjustField::FadeIn, "0.5".into());
    assert_eq!(
        s.adjust_summary().as_deref(),
        Some(
            "Gain \u{2212}3 dB \u{b7} Trim to 1:30 \u{b7} Fade in 0.5 s, out 3 s"
        )
    );
    // A value that does not parse is flagged under its row, not summarised.
    s.set_adjust_text(AdjustField::FadeIn, "x".into());
    assert!(!s.adjust_summary().unwrap().contains("Fade in"));
}

#[test]
fn the_hint_says_what_will_be_written_when_processing_changes_it() {
    // 80 s of 8 kHz stereo.
    let long = entry(8_000, ChannelLayout::stereo(), 8_000 * 80);
    let mut s = ConvertSheet::for_input(&long);
    assert_eq!(s.processing_hint(), None);
    s.set_sample_rate(Some(48_000));
    s.set_channels(Some(parse_channels("mono").unwrap()));
    assert_eq!(
        s.processing_hint().as_deref(),
        Some("48 kHz \u{b7} mono \u{b7} 16-bit \u{b7} 1:20.000")
    );
    s.set_sample_rate(None);
    s.set_channels(None);
    s.set_adjust_text(AdjustField::Start, "20".into());
    s.set_adjust_text(AdjustField::End, "50".into());
    assert_eq!(
        s.processing_hint().as_deref(),
        Some("8 kHz \u{b7} stereo \u{b7} 16-bit \u{b7} 0:30.000")
    );
    s.set_format(FormatChoice::Format(SampleFormat::S24));
    assert_eq!(
        s.processing_hint().as_deref(),
        Some("8 kHz \u{b7} stereo \u{b7} 24-bit \u{b7} 0:30.000")
    );
    // Said once the values are good.
    s.set_adjust_text(AdjustField::End, "5".into());
    assert_eq!(s.processing_hint(), None);
}

#[test]
fn stream_copy_switches_the_controls_off_and_says_why() {
    let mut s = sheet();
    assert!(s.processing_enabled());
    assert_eq!(s.processing_disabled_reason(), None);
    s.set_sample_rate(Some(44_100));
    s.set_adjust_text(AdjustField::FadeOut, "3".into());
    s.set_copy(true);
    assert!(s.copy());
    assert!(!s.processing_enabled());
    let why = s.processing_disabled_reason().unwrap();
    assert!(!why.contains("--"), "{why}");
    assert!(why.contains("copy"), "{why}");
    // Copy passes the packets through: nothing set applies, and nothing is lost.
    let audio = &s.draft().job.outputs[0].audio;
    assert_eq!(audio.sample_rate, None);
    assert!(audio.filters.is_empty());
    assert_eq!(s.processing_hint(), None);
    assert!(s.build(&nothing_exists).is_ok());
    s.set_copy(false);
    assert_eq!(s.sample_rate(), Some(44_100));
    assert_eq!(filters(&s), ["fade out=3s"]);
}

#[test]
fn presets_keep_what_adjust_holds_and_the_preset_row_ignores_it() {
    let mut s = sheet();
    s.set_adjust_text(AdjustField::FadeOut, "3".into());
    s.set_sample_rate(Some(44_100));
    s.apply_preset("cd");
    assert_eq!(filters(&s), ["fade out=3s"]);
    assert_eq!(s.sample_rate(), Some(44_100));
    assert_eq!(s.preset().unwrap().id, "cd");
    // What a preset fixed, changed: Custom, as before.
    s.set_format(FormatChoice::SameAsSource);
    assert!(s.preset().is_none());
    s.apply_preset("flac");
    assert_eq!(filters(&s), ["fade out=3s"]);
    assert_eq!(s.preset().unwrap().id, "flac");
}

#[test]
fn adjust_opens_and_closes_without_touching_the_values() {
    let mut s = sheet();
    s.set_adjust_text(AdjustField::FadeOut, "3".into());
    s.set_adjust_open(true);
    assert!(s.adjust_open());
    s.set_adjust_open(false);
    assert_eq!(filters(&s), ["fade out=3s"]);
}
