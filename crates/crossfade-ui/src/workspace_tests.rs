//! The shell's wiring: elements exist, and clicks and keys reach the workspace.

use crate::actions::AppAction;
use crate::prelude::*;
use crate::test_support::{click, draw, open_test_window, press, step};

fn has(cx: &mut TestAppContext, window: AnyWindowHandle, id: &'static str) -> bool {
    cx.update_window(window, |_, window, _| window.try_find(id).is_some())
        .expect("the window is open")
}

#[gpui_test]
fn the_window_shows_the_toolbar_empty_state_and_status_bar(cx: &mut TestAppContext) {
    let (window, _) = open_test_window(cx);
    draw(cx, window);
    for id in [
        "toolbar-add",
        "toolbar-convert",
        "toolbar-queue",
        "toolbar-settings",
        "toolbar-overflow",
        "empty-state",
        "status-text",
    ] {
        assert!(has(cx, window, id), "missing {id}");
    }
}

#[gpui_test]
fn clicking_add_records_open_files(cx: &mut TestAppContext) {
    let (window, workspace) = open_test_window(cx);
    draw(cx, window);
    click(cx, window, "toolbar-add");
    assert_eq!(
        workspace.read_with(cx, |w, _| w.last_action()),
        Some(AppAction::OpenFiles)
    );
}

#[gpui_test]
fn the_open_shortcut_records_open_files(cx: &mut TestAppContext) {
    let (window, workspace) = open_test_window(cx);
    draw(cx, window);
    press(cx, window, "secondary-o");
    assert_eq!(
        workspace.read_with(cx, |w, _| w.last_action()),
        Some(AppAction::OpenFiles)
    );
}

#[gpui_test]
fn clicking_the_queue_toggle_records_toggle_queue(cx: &mut TestAppContext) {
    let (window, workspace) = open_test_window(cx);
    draw(cx, window);
    click(cx, window, "toolbar-queue");
    assert_eq!(
        workspace.read_with(cx, |w, _| w.last_action()),
        Some(AppAction::ToggleQueue)
    );
}

#[gpui_test]
fn the_overflow_menu_opens_the_keymap_and_about(cx: &mut TestAppContext) {
    let (window, workspace) = open_test_window(cx);
    for (item, expected) in [(0usize, AppAction::OpenKeymap), (1, AppAction::About)] {
        draw(cx, window);
        click(cx, window, "toolbar-overflow");
        step(cx, 2);
        draw(cx, window);
        cx.update_window(window, |_, window, cx| {
            window.within("popup-menu").click(item, cx);
        })
        .expect("the window is open");
        cx.run_until_parked();
        assert_eq!(
            workspace.read_with(cx, |w, _| w.last_action()),
            Some(expected)
        );
    }
}

#[gpui_test]
fn the_default_shortcuts_open_the_sheet_and_toggle_the_queue(cx: &mut TestAppContext) {
    let rig = crate::test_support::open_rig(cx);
    let input = rig.dir.write_wav("in.wav", 48_000, 1, 100);
    crate::test_support::add_paths(cx, &rig, &[&input]);
    press(cx, rig.window, "secondary-enter");
    step(cx, 3);
    draw(cx, rig.window);
    assert!(has(cx, rig.window, "sheet-output"));
    press(cx, rig.window, "escape");
    step(cx, 3);

    press(cx, rig.window, "secondary-j");
    assert!(rig.workspace.read_with(cx, |w, _| w.queue_open()));
    press(cx, rig.window, "secondary-j");
    assert!(!rig.workspace.read_with(cx, |w, _| w.queue_open()));
}
