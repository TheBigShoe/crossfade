//! The convert sheet, driven the way a person drives it, and its parity with the CLI.

use crossfade_app::Request;

use crate::prelude::*;
use crate::test_support::{
    FORMAT_LABELS, PRESET_LABELS, Rig, add_paths, click, disabled, draw, has, open_rig, open_sheet,
    pick, step, type_output,
};

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
        "sheet-format",
        "sheet-output",
        "sheet-browse",
        "sheet-overwrite",
        "sheet-command",
        "sheet-copy",
        "dialog-cancel",
        "dialog-ok",
    ] {
        assert!(has(cx, rig.window, id), "missing {id}");
    }
    assert!(!has(cx, rig.window, "sheet-problems"));
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
    click(cx, rig.window, "sheet-copy");
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
