//! The convert sheet, driven the way a person drives it, and its parity with the CLI.

use crossfade_app::Request;

use crate::fixtures;
use crate::prelude::*;
use crate::test_support::{
    CONTAINER_LABELS, FORMAT_LABELS, LEVEL_LABELS, PRESET_LABELS, Rig, add_paths, click, disabled,
    draw, has, open_rig, open_sheet, pick, pick_label, step, type_output,
};
use crossfade_core::SampleFormat;

/// A rig with `in.wav` opened and selected.
fn rig_with_input(cx: &mut TestAppContext) -> Rig {
    let rig = open_rig(cx);
    let input = rig.dir.write_wav("in.wav", 48_000, 2, 480);
    add_paths(cx, &rig, &[&input]);
    rig
}

fn sheet_output(cx: &mut TestAppContext, rig: &Rig) -> String {
    rig.workspace.read_with(cx, |w, cx| {
        w.convert_dialog()
            .expect("the sheet is open")
            .read(cx)
            .output(cx)
    })
}

fn queued(cx: &mut TestAppContext, rig: &Rig) -> Vec<Request> {
    rig.workspace.read_with(cx, |w, _| {
        w.queue()
            .items()
            .iter()
            .map(|i| i.request.clone())
            .collect()
    })
}

#[gpui_test]
fn convert_opens_a_sheet_titled_with_the_file_and_cancel_closes_it(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    open_sheet(cx, &rig);
    for id in [
        "sheet-preset",
        "sheet-container",
        "sheet-format",
        "sheet-output",
        "sheet-browse",
        "sheet-overwrite",
        "sheet-command",
        "sheet-command-copy",
        "dialog-cancel",
        "dialog-ok",
    ] {
        assert!(has(cx, rig.window, id), "missing {id}");
    }
    assert!(!has(cx, rig.window, "sheet-problems"));
    // Level is FLAC's; the hint appears only when there is something to say.
    for absent in ["sheet-level", "sheet-hint"] {
        assert!(!has(cx, rig.window, absent), "{absent}");
    }
    let title = rig.workspace.read_with(cx, |w, cx| {
        w.convert_dialog()
            .expect("open")
            .read(cx)
            .title()
            .to_owned()
    });
    assert_eq!(title, "Convert in.wav");

    click(cx, rig.window, "dialog-cancel");
    step(cx, 3);
    draw(cx, rig.window);
    assert!(!has(cx, rig.window, "sheet-output"));
    assert!(queued(cx, &rig).is_empty());
}

#[gpui_test]
fn confirming_queues_the_request_opens_the_dock_and_closes_the_sheet(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    open_sheet(cx, &rig);
    click(cx, rig.window, "dialog-ok");
    step(cx, 3);
    draw(cx, rig.window);
    assert!(!has(cx, rig.window, "sheet-output"));
    assert_eq!(queued(cx, &rig).len(), 1);
    assert!(rig.workspace.read_with(cx, |w, _| w.queue_open()));
    // The queue pumped, so the runner has the job.
    assert_eq!(rig.script.starts().len(), 1);
}

#[gpui_test]
fn enter_confirms_and_escape_cancels(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    open_sheet(cx, &rig);
    cx.update_window(rig.window, |_, window, cx| window.press("escape", cx))
        .expect("open");
    step(cx, 3);
    draw(cx, rig.window);
    assert!(!has(cx, rig.window, "sheet-output"));

    open_sheet(cx, &rig);
    click(cx, rig.window, "sheet-output");
    cx.update_window(rig.window, |_, window, cx| window.press("enter", cx))
        .expect("open");
    step(cx, 3);
    draw(cx, rig.window);
    assert_eq!(queued(cx, &rig).len(), 1);
}

#[gpui_test]
fn changing_the_preset_renames_the_default_output_until_the_path_is_edited(
    cx: &mut TestAppContext,
) {
    let rig = rig_with_input(cx);
    open_sheet(cx, &rig);
    assert_eq!(sheet_output(cx, &rig), rig.dir.path("in-copy.wav"));

    // Presets: Same as source, 16-bit (CD), 24-bit, 32-bit float.
    pick(cx, &rig, "sheet-preset", &PRESET_LABELS, 2);
    assert_eq!(sheet_output(cx, &rig), rig.dir.path("in-s24.wav"));

    let mine = rig.dir.path("mine.wav");
    type_output(cx, &rig, &mine);
    assert_eq!(sheet_output(cx, &rig), mine);
    pick(cx, &rig, "sheet-preset", &PRESET_LABELS, 1);
    assert_eq!(sheet_output(cx, &rig), mine);
}

#[gpui_test]
fn the_format_select_and_the_preset_select_stay_in_step(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    open_sheet(cx, &rig);
    pick(cx, &rig, "sheet-preset", &PRESET_LABELS, 1);
    let format = rig.workspace.read_with(cx, |w, cx| {
        w.convert_dialog().expect("open").read(cx).format_label(cx)
    });
    assert_eq!(format, "16-bit integer");

    // f64 has no preset.
    pick(cx, &rig, "sheet-format", &FORMAT_LABELS, 6);
    let preset = rig.workspace.read_with(cx, |w, cx| {
        w.convert_dialog().expect("open").read(cx).preset_label(cx)
    });
    assert_eq!(preset, None);
}

#[gpui_test]
fn an_existing_output_blocks_confirm_until_replace_is_on(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    let taken = rig.dir.write("taken.wav", b"already here");
    open_sheet(cx, &rig);
    type_output(cx, &rig, &taken);
    assert!(has(cx, rig.window, "sheet-problems"));
    assert!(disabled(cx, rig.window, "dialog-ok"));

    click(cx, rig.window, "sheet-overwrite");
    draw(cx, rig.window);
    assert!(!has(cx, rig.window, "sheet-problems"));
    assert!(!disabled(cx, rig.window, "dialog-ok"));
}

#[gpui_test]
fn the_command_line_stays_visible_while_the_sheet_has_a_problem(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    let taken = rig.dir.write("taken.wav", b"already here");
    open_sheet(cx, &rig);
    assert!(has(cx, rig.window, "sheet-command"));
    type_output(cx, &rig, &taken);
    assert!(has(cx, rig.window, "sheet-problems"));
    assert!(
        has(cx, rig.window, "sheet-command"),
        "only the primary button is disabled"
    );
    assert!(disabled(cx, rig.window, "dialog-ok"));
}

#[gpui_test]
fn an_output_equal_to_the_input_stays_blocked_even_with_replace_on(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    open_sheet(cx, &rig);
    type_output(cx, &rig, &rig.dir.path("in.wav"));
    click(cx, rig.window, "sheet-overwrite");
    draw(cx, rig.window);
    assert!(has(cx, rig.window, "sheet-problems"));
    assert!(disabled(cx, rig.window, "dialog-ok"));
    let problems = rig.workspace.read_with(cx, |w, cx| {
        w.convert_dialog()
            .expect("open")
            .read(cx)
            .problem_messages(cx)
    });
    assert_eq!(problems, ["The output would replace the input file."]);
}

#[gpui_test]
fn a_non_wav_output_is_blocked(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    open_sheet(cx, &rig);
    type_output(cx, &rig, &rig.dir.path("out.mp3"));
    assert!(disabled(cx, rig.window, "dialog-ok"));
}

#[gpui_test]
fn browse_asks_for_a_save_path_and_puts_it_in_the_field(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    open_sheet(cx, &rig);
    click(cx, rig.window, "sheet-browse");
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|dir| Some(dir.join("picked.wav")));
    cx.run_until_parked();
    draw(cx, rig.window);
    assert_eq!(sheet_output(cx, &rig), rig.dir.path("picked.wav"));
}

#[gpui_test]
fn the_copy_button_puts_the_equivalent_command_on_the_clipboard(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    open_sheet(cx, &rig);
    pick(cx, &rig, "sheet-preset", &PRESET_LABELS, 1);
    click(cx, rig.window, "sheet-command-copy");
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    let expected = format!(
        "crossfade convert {} -o {} --sample-format s16",
        rig.dir.path("in.wav"),
        rig.dir.path("in-s16.wav")
    );
    assert_eq!(copied, Some(expected));
}

/// What a person wants from the sheet, and the words they would type to the CLI for it.
struct Intent {
    /// The output's file name when typed; `None` keeps the default the sheet offers.
    output: Option<&'static str>,
    /// An item of the preset select.
    preset: Option<usize>,
    /// An item of the format select (0 is Same as source).
    format: Option<usize>,
    overwrite: bool,
    /// The file the CLI would be told to write.
    expect_output: &'static str,
    /// The CLI's `--sample-format`, if any.
    sample_format: Option<&'static str>,
    /// Pre-existing output, so replacing it is meaningful.
    existing: bool,
}

const INTENTS: &[Intent] = &[
    Intent {
        output: Some("plain.wav"),
        preset: None,
        format: None,
        overwrite: false,
        expect_output: "plain.wav",
        sample_format: None,
        existing: false,
    },
    Intent {
        output: None,
        preset: None,
        format: None,
        overwrite: false,
        expect_output: "in-copy.wav",
        sample_format: None,
        existing: false,
    },
    Intent {
        output: Some("cd.wav"),
        preset: Some(1),
        format: None,
        overwrite: false,
        expect_output: "cd.wav",
        sample_format: Some("s16"),
        existing: false,
    },
    Intent {
        output: Some("studio.wav"),
        preset: Some(2),
        format: None,
        overwrite: false,
        expect_output: "studio.wav",
        sample_format: Some("s24"),
        existing: false,
    },
    Intent {
        output: Some("float.wav"),
        preset: Some(3),
        format: None,
        overwrite: false,
        expect_output: "float.wav",
        sample_format: Some("f32"),
        existing: false,
    },
    Intent {
        output: Some("eight.wav"),
        preset: None,
        format: Some(1),
        overwrite: false,
        expect_output: "eight.wav",
        sample_format: Some("u8"),
        existing: false,
    },
    Intent {
        output: Some("wide.wav"),
        preset: None,
        format: Some(4),
        overwrite: false,
        expect_output: "wide.wav",
        sample_format: Some("s32"),
        existing: false,
    },
    Intent {
        output: Some("double.wav"),
        preset: None,
        format: Some(6),
        overwrite: false,
        expect_output: "double.wav",
        sample_format: Some("f64"),
        existing: false,
    },
    Intent {
        output: Some("again.wav"),
        preset: None,
        format: None,
        overwrite: true,
        expect_output: "again.wav",
        sample_format: None,
        existing: true,
    },
    Intent {
        output: Some("again24.wav"),
        preset: Some(2),
        format: None,
        overwrite: true,
        expect_output: "again24.wav",
        sample_format: Some("s24"),
        existing: true,
    },
    Intent {
        output: None,
        preset: Some(1),
        format: None,
        overwrite: false,
        expect_output: "in-s16.wav",
        sample_format: Some("s16"),
        existing: false,
    },
    Intent {
        output: Some("back.wav"),
        preset: Some(2),
        format: Some(0),
        overwrite: false,
        expect_output: "back.wav",
        sample_format: None,
        existing: false,
    },
];

#[gpui_test]
fn the_sheet_builds_what_the_cli_builds_for_the_same_intent(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    for (n, intent) in INTENTS.iter().enumerate() {
        if intent.existing {
            rig.dir.write(intent.expect_output, b"old");
        }
        open_sheet(cx, &rig);
        if let Some(preset) = intent.preset {
            pick(cx, &rig, "sheet-preset", &PRESET_LABELS, preset);
        }
        if let Some(format) = intent.format {
            pick(cx, &rig, "sheet-format", &FORMAT_LABELS, format);
        }
        if let Some(output) = intent.output {
            type_output(cx, &rig, &rig.dir.path(output));
        }
        if intent.overwrite {
            click(cx, rig.window, "sheet-overwrite");
            draw(cx, rig.window);
        }
        click(cx, rig.window, "dialog-ok");
        step(cx, 3);
        draw(cx, rig.window);

        let mut args = vec![
            "convert".to_owned(),
            rig.dir.path("in.wav"),
            "-o".to_owned(),
            rig.dir.path(intent.expect_output),
        ];
        if let Some(format) = intent.sample_format {
            args.extend(["--sample-format".to_owned(), format.to_owned()]);
        }
        if intent.overwrite {
            args.push("--force".to_owned());
        }
        let cli = crossfade_cli::request_from_args(&args)
            .expect("valid arguments")
            .expect("a convert request");
        let built = queued(cx, &rig);
        assert_eq!(built.len(), n + 1, "intent {n} was not queued");
        assert_eq!(built[n], cli, "intent {n}: {args:?}");
    }
}

/// A rig with `name` holding `bytes` opened and selected.
fn rig_with_file(cx: &mut TestAppContext, name: &str, bytes: &[u8]) -> Rig {
    let rig = open_rig(cx);
    let input = rig.dir.write(name, bytes);
    add_paths(cx, &rig, &[&input]);
    rig
}

fn dialog<R>(
    cx: &mut TestAppContext,
    rig: &Rig,
    read: impl FnOnce(&crate::ConvertDialog, &App) -> R,
) -> R {
    rig.workspace.read_with(cx, |w, cx| {
        read(w.convert_dialog().expect("the sheet is open").read(cx), cx)
    })
}

fn labels(cx: &mut TestAppContext, rig: &Rig, id: &'static str) -> Vec<String> {
    dialog(cx, rig, |d, cx| d.select_labels(id, cx))
}

#[gpui_test]
fn the_container_select_rewrites_the_extension_and_filters_the_formats(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    open_sheet(cx, &rig);
    assert_eq!(labels(cx, &rig, "sheet-container"), CONTAINER_LABELS);
    assert_eq!(labels(cx, &rig, "sheet-format"), FORMAT_LABELS);

    pick(cx, &rig, "sheet-container", &CONTAINER_LABELS, 1);
    assert_eq!(sheet_output(cx, &rig), rig.dir.path("in.flac"));
    // FLAC holds whole numbers up to 24 bits.
    assert_eq!(labels(cx, &rig, "sheet-format"), &FORMAT_LABELS[..4]);

    // A typed path keeps its folder and stem; only the extension follows.
    let mine = rig.dir.path("my take.v2.flac");
    type_output(cx, &rig, &mine);
    pick(cx, &rig, "sheet-container", &CONTAINER_LABELS, 2);
    assert_eq!(sheet_output(cx, &rig), rig.dir.path("my take.v2.aiff"));
    assert_eq!(labels(cx, &rig, "sheet-format"), FORMAT_LABELS);

    // And typing an extension switches the select.
    type_output(cx, &rig, &rig.dir.path("again.flac"));
    assert_eq!(dialog(cx, &rig, |d, cx| d.container_label(cx)), "FLAC");
    assert_eq!(labels(cx, &rig, "sheet-format"), &FORMAT_LABELS[..4]);
}

#[gpui_test]
fn a_float_source_into_flac_shows_the_hint_and_is_saved_as_24_bit(cx: &mut TestAppContext) {
    let wav = fixtures::wav(48_000, 2, SampleFormat::F32, 480);
    let rig = rig_with_file(cx, "float.wav", &wav);
    open_sheet(cx, &rig);
    assert!(!has(cx, rig.window, "sheet-hint"));
    pick(cx, &rig, "sheet-container", &CONTAINER_LABELS, 1);
    assert!(has(cx, rig.window, "sheet-hint"));
    let hint = dialog(cx, &rig, |d, _| d.hint());
    assert_eq!(
        hint.as_deref(),
        Some("FLAC stores whole-number samples up to 24-bit, so this will be saved as 24-bit.")
    );
    assert_eq!(
        dialog(cx, &rig, |d, cx| d.format_label(cx)),
        "24-bit integer"
    );
    assert_eq!(labels(cx, &rig, "sheet-format"), &FORMAT_LABELS[1..4]);
    assert!(!disabled(cx, rig.window, "dialog-ok"));

    click(cx, rig.window, "dialog-ok");
    step(cx, 3);
    let audio = queued(cx, &rig)[0].job.outputs[0].audio.clone();
    assert_eq!(audio.sample_format, Some(SampleFormat::S24));
    assert_eq!(audio.codec, None);

    // Going back to WAV takes the hint away.
    open_sheet(cx, &rig);
    pick(cx, &rig, "sheet-container", &CONTAINER_LABELS, 1);
    pick(cx, &rig, "sheet-container", &CONTAINER_LABELS, 0);
    assert!(!has(cx, rig.window, "sheet-hint"));
}

#[gpui_test]
fn a_192_khz_source_cannot_be_queued_as_flac(cx: &mut TestAppContext) {
    let wav = fixtures::wav(192_000, 1, SampleFormat::S24, 480);
    let rig = rig_with_file(cx, "hi.wav", &wav);
    open_sheet(cx, &rig);
    assert!(!disabled(cx, rig.window, "dialog-ok"));
    pick(cx, &rig, "sheet-container", &CONTAINER_LABELS, 1);
    assert!(has(cx, rig.window, "sheet-problems"));
    assert!(disabled(cx, rig.window, "dialog-ok"));
    let problems = dialog(cx, &rig, |d, cx| d.problem_messages(cx));
    assert_eq!(
        problems,
        ["FLAC output is limited to 96 kHz for now; this file is 192 kHz."]
    );
    assert!(problems.iter().all(|p| !p.contains("--")));
    assert!(!has(cx, rig.window, "sheet-hint"));
    pick(cx, &rig, "sheet-container", &CONTAINER_LABELS, 0);
    assert!(!has(cx, rig.window, "sheet-problems"));
}

#[gpui_test]
fn the_level_select_exists_for_flac_only(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    open_sheet(cx, &rig);
    assert!(!has(cx, rig.window, "sheet-level"));
    pick(cx, &rig, "sheet-container", &CONTAINER_LABELS, 1);
    assert!(has(cx, rig.window, "sheet-level"));
    assert_eq!(labels(cx, &rig, "sheet-level"), LEVEL_LABELS);
    pick(cx, &rig, "sheet-level", &LEVEL_LABELS, 9);
    click(cx, rig.window, "dialog-ok");
    step(cx, 3);
    assert_eq!(
        queued(cx, &rig)[0].job.outputs[0].audio.compression_level,
        Some(8)
    );

    open_sheet(cx, &rig);
    pick(cx, &rig, "sheet-container", &CONTAINER_LABELS, 1);
    pick(cx, &rig, "sheet-level", &LEVEL_LABELS, 1);
    pick(cx, &rig, "sheet-container", &CONTAINER_LABELS, 2);
    assert!(!has(cx, rig.window, "sheet-level"));
    pick(cx, &rig, "sheet-container", &CONTAINER_LABELS, 1);
    assert_eq!(dialog(cx, &rig, |d, cx| d.level_label(cx)), "Default");
}

#[gpui_test]
fn copy_is_offered_for_a_flac_source_and_replaces_the_format_and_level(cx: &mut TestAppContext) {
    let flac = fixtures::flac(44_100, 2, SampleFormat::S16, 4_410);
    let rig = rig_with_file(cx, "tone.flac", &flac);
    open_sheet(cx, &rig);
    // The sheet starts as WAV, which cannot hold FLAC packets.
    assert!(!has(cx, rig.window, "sheet-copy"));
    pick(cx, &rig, "sheet-container", &CONTAINER_LABELS, 1);
    assert!(has(cx, rig.window, "sheet-copy"));
    assert!(dialog(cx, &rig, |d, _| d.format_enabled()));

    click(cx, rig.window, "sheet-copy");
    draw(cx, rig.window);
    assert!(dialog(cx, &rig, |d, _| d.copy()));
    assert!(!dialog(cx, &rig, |d, _| d.format_enabled()));
    assert!(!dialog(cx, &rig, |d, _| d.level_enabled()));
    assert_eq!(sheet_output(cx, &rig), rig.dir.path("tone-copy.flac"));

    click(cx, rig.window, "dialog-ok");
    step(cx, 3);
    let audio = queued(cx, &rig)[0].job.outputs[0].audio.clone();
    assert_eq!(audio.codec.as_deref(), Some("copy"));
    assert_eq!((audio.sample_format, audio.compression_level), (None, None));
}

#[gpui_test]
fn a_wav_source_offers_copy_into_wav_only(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    open_sheet(cx, &rig);
    assert!(has(cx, rig.window, "sheet-copy"));
    pick(cx, &rig, "sheet-container", &CONTAINER_LABELS, 2);
    assert!(!has(cx, rig.window, "sheet-copy"));
}

#[gpui_test]
fn the_command_line_preview_follows_container_level_and_copy(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    open_sheet(cx, &rig);
    pick(cx, &rig, "sheet-container", &CONTAINER_LABELS, 1);
    pick(cx, &rig, "sheet-level", &LEVEL_LABELS, 1);
    click(cx, rig.window, "sheet-command-copy");
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(
        copied,
        Some(format!(
            "crossfade convert {} -o {} --compression-level 0",
            rig.dir.path("in.wav"),
            rig.dir.path("in.flac")
        ))
    );
}

#[gpui_test]
fn an_unsaveable_extension_is_explained_in_plain_words(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    open_sheet(cx, &rig);
    type_output(cx, &rig, &rig.dir.path("out.ogg"));
    assert!(disabled(cx, rig.window, "dialog-ok"));
    let problems = dialog(cx, &rig, |d, cx| d.problem_messages(cx));
    assert_eq!(
        problems,
        [
            "Crossfade can save .wav, .flac and .aiff files. Ogg files can be opened but not saved yet."
        ]
    );
}

/// What a person does to a freshly opened sheet.
#[derive(Clone, Copy)]
enum Act {
    Container(&'static str),
    Level(usize),
    Preset(usize),
    Format(&'static str),
    Copy,
    Type(&'static str),
}

struct Intent2 {
    steps: &'static [Act],
    /// What they would type after `convert <input>`; `@name` is a path in the scratch folder.
    cli: &'static [&'static str],
}

const WAV_INTENTS: &[Intent2] = &[
    Intent2 {
        steps: &[Act::Container("FLAC")],
        cli: &["-o", "@in.flac"],
    },
    Intent2 {
        steps: &[Act::Container("FLAC"), Act::Level(1)],
        cli: &["-o", "@in.flac", "--compression-level", "0"],
    },
    Intent2 {
        steps: &[Act::Container("FLAC"), Act::Level(9)],
        cli: &["-o", "@in.flac", "--compression-level", "8"],
    },
    Intent2 {
        steps: &[Act::Preset(5)],
        cli: &["-o", "@in-s16.flac", "--sample-format", "s16"],
    },
    Intent2 {
        steps: &[Act::Preset(6)],
        cli: &["-o", "@in-s24.flac", "--sample-format", "s24"],
    },
    Intent2 {
        steps: &[Act::Preset(7)],
        cli: &["-o", "@in.aiff"],
    },
    Intent2 {
        steps: &[Act::Container("AIFF"), Act::Format("24-bit integer")],
        cli: &["-o", "@in-s24.aiff", "--sample-format", "s24"],
    },
    Intent2 {
        steps: &[Act::Type("mine.wav"), Act::Container("FLAC")],
        cli: &["-o", "@mine.flac"],
    },
    Intent2 {
        steps: &[Act::Type("x.flac"), Act::Level(3)],
        cli: &["-o", "@x.flac", "--compression-level", "2"],
    },
    Intent2 {
        steps: &[Act::Container("FLAC"), Act::Level(6), Act::Container("WAV")],
        cli: &["-o", "@in-copy.wav"],
    },
    Intent2 {
        steps: &[Act::Copy],
        cli: &["-o", "@in-copy.wav", "--codec", "copy"],
    },
];

const FLAC_INTENTS: &[Intent2] = &[
    Intent2 {
        steps: &[Act::Container("FLAC"), Act::Copy],
        cli: &["-o", "@in-copy.flac", "--codec", "copy"],
    },
    Intent2 {
        steps: &[Act::Type("out.flac"), Act::Copy],
        cli: &["-o", "@out.flac", "--codec", "copy"],
    },
    Intent2 {
        steps: &[Act::Preset(1)],
        cli: &["-o", "@in-s16.wav", "--sample-format", "s16"],
    },
];

fn run_intents(cx: &mut TestAppContext, input: &str, bytes: &[u8], intents: &[Intent2]) {
    let rig = rig_with_file(cx, input, bytes);
    for (n, intent) in intents.iter().enumerate() {
        open_sheet(cx, &rig);
        for act in intent.steps {
            match *act {
                Act::Container(label) => pick_label(cx, &rig, "sheet-container", label),
                Act::Level(ix) => pick(cx, &rig, "sheet-level", &LEVEL_LABELS, ix),
                Act::Preset(ix) => pick(cx, &rig, "sheet-preset", &PRESET_LABELS, ix),
                Act::Format(label) => pick_label(cx, &rig, "sheet-format", label),
                Act::Copy => {
                    click(cx, rig.window, "sheet-copy");
                    draw(cx, rig.window);
                }
                Act::Type(name) => type_output(cx, &rig, &rig.dir.path(name)),
            }
        }
        click(cx, rig.window, "dialog-ok");
        step(cx, 3);
        draw(cx, rig.window);

        let mut args = vec!["convert".to_owned(), rig.dir.path(input)];
        args.extend(intent.cli.iter().map(|a| match a.strip_prefix('@') {
            Some(name) => rig.dir.path(name),
            None => (*a).to_owned(),
        }));
        let cli = crossfade_cli::request_from_args(&args)
            .expect("valid arguments")
            .expect("a convert request");
        let built = queued(cx, &rig);
        assert_eq!(built.len(), n + 1, "{input} intent {n} was not queued");
        assert_eq!(built[n], cli, "{input} intent {n}: {args:?}");
    }
}

#[gpui_test]
fn wav_sheet_intents_equal_the_cli_requests(cx: &mut TestAppContext) {
    let wav = fixtures::wav(48_000, 2, SampleFormat::S16, 480);
    run_intents(cx, "in.wav", &wav, WAV_INTENTS);
}

#[gpui_test]
fn flac_sheet_intents_equal_the_cli_requests(cx: &mut TestAppContext) {
    let flac = fixtures::flac(44_100, 2, SampleFormat::S16, 4_410);
    run_intents(cx, "in.flac", &flac, FLAC_INTENTS);
}
