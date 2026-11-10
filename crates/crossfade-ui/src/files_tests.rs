//! The files list, the inspector and the ways files get in: add, drop, remove, select.

use crossfade_app::inputs::InputState;
use crossfade_app::inspector;

use crate::prelude::*;
use crate::test_support::{add_paths, click, disabled, draw, has, open_rig, open_test_window};
use crate::workspace::Workspace;

fn row_id(prefix: &str, n: usize) -> SharedString {
    format!("{prefix}-{n}").into()
}

fn selected_name(cx: &mut TestAppContext, workspace: &Entity<Workspace>) -> Option<String> {
    workspace.read_with(cx, |w, _| w.inputs().selected().map(|e| e.name.clone()))
}

#[gpui_test]
fn adding_two_files_lists_both_and_selects_the_last(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let a = rig.dir.write_wav("a.wav", 48_000, 2, 480);
    let b = rig.dir.write_wav("b.wav", 44_100, 1, 441);
    add_paths(cx, &rig, &[&a, &b]);

    assert!(has(cx, rig.window, "files-panel"));
    assert!(has(cx, rig.window, row_id("file-row", 0)));
    assert!(has(cx, rig.window, row_id("file-row", 1)));
    assert!(!has(cx, rig.window, row_id("file-row", 2)));
    assert!(!has(cx, rig.window, "empty-state"));
    assert_eq!(selected_name(cx, &rig.workspace).as_deref(), Some("b.wav"));
    assert_eq!(rig.workspace.read_with(cx, |w, _| w.status_text()), "b.wav");
}

#[gpui_test]
fn the_inspector_shows_what_the_model_returns_for_the_selected_file(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let a = rig.dir.write_wav("a.wav", 48_000, 2, 480);
    let b = rig.dir.write_wav("b.wav", 44_100, 1, 441);
    add_paths(cx, &rig, &[&a, &b]);

    let expected = rig.workspace.read_with(cx, |w, _| {
        match &w.inputs().selected().expect("selected").state {
            InputState::Probed(doc, _) => inspector::sections(doc),
            InputState::Failed(error) => panic!("probe failed: {error}"),
        }
    });
    let shown = rig.workspace.read_with(cx, |w, _| w.inspector_sections());
    assert_eq!(shown, expected);
    for (s, section) in expected.iter().enumerate() {
        for row in &section.rows {
            let id: SharedString = format!("inspector-row-{s}-{}", row.field).into();
            assert!(has(cx, rig.window, id.clone()), "missing {id}");
        }
    }

    // Selecting the other file swaps the rows for its own.
    click(cx, rig.window, row_id("file-row", 0));
    draw(cx, rig.window);
    let first = rig.workspace.read_with(cx, |w, _| w.inspector_sections());
    assert_ne!(first, expected);
    assert_eq!(selected_name(cx, &rig.workspace).as_deref(), Some("a.wav"));
}

#[gpui_test]
fn a_failed_probe_shows_the_alert_row_and_the_error(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let bad = rig.dir.write("bad.wav", b"this is not audio");
    add_paths(cx, &rig, &[&bad]);

    assert!(has(cx, rig.window, "inspector-error"));
    assert!(has(cx, rig.window, row_id("file-row", 0)));
    assert!(
        rig.workspace
            .read_with(cx, |w, _| w.inspector_error().is_some())
    );
    assert!(
        rig.workspace
            .read_with(cx, |w, _| w.inspector_sections().is_empty())
    );
    let summary = rig.workspace.read_with(cx, |w, _| w.file_summary(0));
    assert_eq!(summary, "unreadable");
}

#[gpui_test]
fn a_good_file_summary_is_one_muted_line(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let a = rig.dir.write_wav("a.wav", 48_000, 2, 48_000 * 62 + 12_000);
    add_paths(cx, &rig, &[&a]);
    let summary = rig.workspace.read_with(cx, |w, _| w.file_summary(0));
    assert_eq!(summary, "48 kHz \u{b7} stereo \u{b7} s16 \u{b7} 1:02.250");
}

#[gpui_test]
fn removing_a_file_moves_the_selection_as_the_model_does(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let names = ["a.wav", "b.wav", "c.wav"];
    let paths: Vec<String> = names
        .iter()
        .map(|n| rig.dir.write_wav(n, 48_000, 1, 100))
        .collect();
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    add_paths(cx, &rig, &refs);
    assert_eq!(selected_name(cx, &rig.workspace).as_deref(), Some("c.wav"));

    // The remove button shows on hover; hovering first mirrors what a user does.
    cx.update_window(rig.window, |_, window, cx| {
        window.hover(row_id("file-row", 2), cx);
    })
    .expect("open");
    click(cx, rig.window, row_id("file-remove", 2));
    draw(cx, rig.window);
    assert_eq!(selected_name(cx, &rig.workspace).as_deref(), Some("b.wav"));
    assert!(!has(cx, rig.window, row_id("file-row", 2)));

    for _ in 0..2 {
        cx.update_window(rig.window, |_, window, cx| {
            window.hover(row_id("file-row", 0), cx);
        })
        .expect("open");
        click(cx, rig.window, row_id("file-remove", 0));
        draw(cx, rig.window);
    }
    assert_eq!(selected_name(cx, &rig.workspace), None);
    assert!(has(cx, rig.window, "empty-state"));
    assert_eq!(rig.workspace.read_with(cx, |w, _| w.status_text()), "Ready");
}

#[gpui_test]
fn convert_is_disabled_without_a_probed_selection(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    draw(cx, rig.window);
    assert!(disabled(cx, rig.window, "toolbar-convert"));

    let bad = rig.dir.write("bad.wav", b"nope");
    add_paths(cx, &rig, &[&bad]);
    assert!(disabled(cx, rig.window, "toolbar-convert"));

    let good = rig.dir.write_wav("ok.wav", 48_000, 1, 100);
    add_paths(cx, &rig, &[&good]);
    assert!(!disabled(cx, rig.window, "toolbar-convert"));
}

#[gpui_test]
fn files_are_listed_in_the_order_given_and_a_repeat_does_not_duplicate(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let a = rig.dir.write_wav("a.wav", 48_000, 1, 100);
    let b = rig.dir.write_wav("b.wav", 48_000, 1, 100);
    add_paths(cx, &rig, &[&b, &a, &b]);
    let names: Vec<String> = rig.workspace.read_with(cx, |w, _| {
        w.inputs()
            .entries()
            .iter()
            .map(|e| e.name.clone())
            .collect()
    });
    assert_eq!(names, ["b.wav", "a.wav"]);
    assert_eq!(selected_name(cx, &rig.workspace).as_deref(), Some("b.wav"));
}

#[gpui_test]
fn the_open_action_asks_the_platform_for_files_and_adds_the_answer(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let a = rig.dir.write_wav("a.wav", 48_000, 2, 480);
    let b = rig.dir.write_wav("b.wav", 44_100, 1, 441);
    draw(cx, rig.window);
    click(cx, rig.window, "toolbar-add");
    assert!(cx.did_prompt_for_paths());

    let answer = vec![std::path::PathBuf::from(&a), std::path::PathBuf::from(&b)];
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && options.multiple, "{options:?}");
        Some(answer)
    });
    cx.run_until_parked();
    draw(cx, rig.window);
    assert!(has(cx, rig.window, row_id("file-row", 1)));
    assert_eq!(selected_name(cx, &rig.workspace).as_deref(), Some("b.wav"));
}

#[gpui_test]
fn cancelling_the_open_prompt_changes_nothing(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    draw(cx, rig.window);
    click(cx, rig.window, "toolbar-add");
    cx.simulate_path_prompt_response(|_| None);
    cx.run_until_parked();
    assert!(rig.workspace.read_with(cx, |w, _| w.inputs().is_empty()));
}

#[gpui_test]
fn dropping_files_on_the_window_adds_them_and_highlights_while_over(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let a = rig.dir.write_wav("a.wav", 48_000, 2, 480);
    draw(cx, rig.window);
    let over = point(px(400.), px(300.));
    let paths = ExternalPaths(std::iter::once(std::path::PathBuf::from(&a)).collect());

    cx.update_window(rig.window, |_, window, cx| {
        window.dispatch_event(
            PlatformInput::FileDrop(FileDropEvent::Entered {
                position: over,
                paths,
            }),
            cx,
        );
        window.render_frame(cx);
    })
    .expect("open");
    assert!(has(cx, rig.window, "drop-highlight"));

    cx.update_window(rig.window, |_, window, cx| {
        window.dispatch_event(
            PlatformInput::FileDrop(FileDropEvent::Submit { position: over }),
            cx,
        );
    })
    .expect("open");
    cx.run_until_parked();
    draw(cx, rig.window);
    assert_eq!(selected_name(cx, &rig.workspace).as_deref(), Some("a.wav"));
    assert!(!has(cx, rig.window, "drop-highlight"));
}

#[gpui_test]
fn a_directory_of_nothing_still_shows_the_empty_state(cx: &mut TestAppContext) {
    let (window, workspace) = open_test_window(cx);
    draw(cx, window);
    assert!(has(cx, window, "empty-state"));
    assert!(!has(cx, window, "files-panel"));
    assert_eq!(workspace.read_with(cx, |w, _| w.status_text()), "Ready");
}

fn size_of(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    id: impl Into<ElementId>,
) -> Size<Pixels> {
    let id = id.into();
    cx.update_window(window, |_, window, _| window.find(id).bounds().size)
        .expect("the window is open")
}

#[gpui_test]
fn file_rows_are_two_lines_with_the_full_width_for_the_name(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let long = rig
        .dir
        .write_wav("interview-take-one-with-a-long-name.wav", 48_000, 2, 480);
    add_paths(cx, &rig, &[&long]);
    let row = cx
        .update_window(rig.window, |_, window, _| {
            window.find(row_id("file-row", 0)).bounds()
        })
        .unwrap();
    let summary = cx
        .update_window(rig.window, |_, window, _| {
            window.find(row_id("file-summary", 0)).bounds()
        })
        .unwrap();
    // The second line keeps clear of the next row.
    assert!(
        row.bottom() - summary.bottom() >= px(4.),
        "summary ends {:?} above the row's bottom",
        row.bottom() - summary.bottom()
    );
    assert_eq!(summary.size.height, px(16.));
    let name = cx
        .update_window(rig.window, |_, window, _| {
            window.find(row_id("file-name", 0)).bounds()
        })
        .unwrap();
    let summary = cx
        .update_window(rig.window, |_, window, _| {
            window.find(row_id("file-summary", 0)).bounds()
        })
        .unwrap();
    assert!(
        summary.origin.y >= name.origin.y + name.size.height,
        "the summary is the second line"
    );
    // The row is 260 wide; icon, padding and the gaps leave the rest to the name.
    assert!(
        name.size.width > px(150.),
        "name width {:?}",
        name.size.width
    );
    assert!(
        summary.size.width >= name.size.width - px(1.),
        "the summary has the same room"
    );
}

#[gpui_test]
fn inspector_rows_are_24_px_tall(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let a = rig.dir.write_wav("a.wav", 48_000, 2, 480);
    add_paths(cx, &rig, &[&a]);
    let sections = rig.workspace.read_with(cx, |w, _| w.inspector_sections());
    let row = &sections[0].rows[0];
    let id: SharedString = format!("inspector-row-0-{}", row.field).into();
    assert_eq!(size_of(cx, rig.window, id).height, px(24.));
}

#[gpui_test]
fn a_selected_row_is_clearly_distinct_from_a_hovered_one_in_both_themes(cx: &mut TestAppContext) {
    let _rig = open_rig(cx);
    for dark in [true, false] {
        cx.update(|cx| crate::config::set_appearance(dark, cx));
        cx.update(|cx| {
            let (selected, hover) = crate::files::row_colors(cx);
            let background = cx.theme().background.to_rgb();
            let flat = |top: Hsla| {
                let top = top.to_rgb();
                let mix = |t: f32, u: f32| t * top.a + u * (1.0 - top.a);
                [
                    mix(top.r, background.r),
                    mix(top.g, background.g),
                    mix(top.b, background.b),
                ]
            };
            let (s, h) = (flat(selected), flat(hover));
            let distance = s
                .iter()
                .zip(h)
                .map(|(s, h)| (s - h).powi(2))
                .sum::<f32>()
                .sqrt();
            assert!(
                distance > 0.08,
                "dark={dark}: selected vs hover colour distance {distance}"
            );
        });
    }
}
