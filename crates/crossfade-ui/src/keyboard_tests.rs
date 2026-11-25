//! Keyboard use: the files list, the buttons, and where Tab goes.

use crossfade_app::player::State as PlayState;

use crate::prelude::*;
use crate::test_support::{Rig, add_paths, click, draw, open_rig, press};

fn three_files(cx: &mut TestAppContext) -> Rig {
    let rig = open_rig(cx);
    let paths: Vec<String> = ["a.wav", "b.wav", "c.wav"]
        .iter()
        .map(|name| rig.dir.write_wav(name, 48_000, 2, 48_000))
        .collect();
    let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
    add_paths(cx, &rig, &paths);
    draw(cx, rig.window);
    rig
}

fn selected(cx: &mut TestAppContext, rig: &Rig) -> Option<String> {
    rig.workspace
        .read_with(cx, |w, _| w.inputs().selected().map(|e| e.name.clone()))
}

fn focused(cx: &mut TestAppContext, rig: &Rig, id: &'static str) -> bool {
    cx.update_window(rig.window, |_, window, _| {
        window.try_find(id).and_then(|e| e.focused()) == Some(true)
    })
    .unwrap()
}

/// Moves focus with Tab until `id` has it; false when a whole round never reached it.
fn tab_to(cx: &mut TestAppContext, rig: &Rig, id: &'static str) -> bool {
    for _ in 0..40 {
        if focused(cx, rig, id) {
            return true;
        }
        press(cx, rig.window, "tab");
    }
    false
}

#[gpui_test]
fn up_and_down_move_the_files_selection_when_the_list_has_focus(cx: &mut TestAppContext) {
    let rig = three_files(cx);
    click(cx, rig.window, "file-row-0");
    draw(cx, rig.window);
    assert!(focused(cx, &rig, "files-list"), "a click focuses the list");
    assert_eq!(selected(cx, &rig).as_deref(), Some("a.wav"));
    press(cx, rig.window, "down");
    press(cx, rig.window, "down");
    assert_eq!(selected(cx, &rig).as_deref(), Some("c.wav"));
    press(cx, rig.window, "up");
    assert_eq!(selected(cx, &rig).as_deref(), Some("b.wav"));
    // The strip follows the selection, as a click does.
    let shown = rig
        .workspace
        .read_with(cx, |w, cx| w.strip().read(cx).path().map(str::to_owned));
    assert!(shown.is_some_and(|p| p.ends_with("b.wav")));
}

#[gpui_test]
fn space_on_a_focused_toolbar_button_presses_it_and_leaves_playback_alone(cx: &mut TestAppContext) {
    let rig = three_files(cx);
    assert!(tab_to(cx, &rig, "toolbar-queue"), "Tab reaches the button");
    let open = rig.workspace.read_with(cx, |w, _| w.queue_open());
    press(cx, rig.window, "space");
    assert_ne!(
        rig.workspace.read_with(cx, |w, _| w.queue_open()),
        open,
        "Space pressed the button"
    );
    let state = rig
        .workspace
        .read_with(cx, |w, cx| w.strip().read(cx).player_state());
    assert_ne!(state, Some(PlayState::Playing), "and did not play");
}

#[gpui_test]
fn tab_reaches_the_files_list_the_strip_and_the_queue(cx: &mut TestAppContext) {
    let rig = three_files(cx);
    click(cx, rig.window, "toolbar-queue");
    draw(cx, rig.window);
    for id in ["files-list", "waveform-strip", "queue-clear"] {
        assert!(tab_to(cx, &rig, id), "Tab never reached {id}");
    }
}
