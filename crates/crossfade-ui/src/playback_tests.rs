//! Preview playback on the waveform strip: the wiring from buttons, keys and the pointer to the
//! `crossfade-app` player, over the scripted (virtual) audio output. The behaviour itself is
//! tested in `crossfade-app`'s `tests/player.rs` and `tests/follow.rs`.

use std::sync::mpsc;

use crossfade_app::FsIo;
use crossfade_app::analysis::{Analyzer, State as AnalysisState};
use crossfade_app::player::State;
use crossfade_core::Limits;

use crate::prelude::*;
use crate::test_support::{
    Rig, add_paths, click, draw, has, open_rig, open_sheet, open_with, press,
};
use crate::waveform_view::WaveformStrip;
use crate::workspace::{AudioOutput, WorkspaceOptions};

fn strip(cx: &mut TestAppContext, rig: &Rig) -> Entity<WaveformStrip> {
    rig.workspace.read_with(cx, |w, _| w.strip().clone())
}

fn state(cx: &mut TestAppContext, rig: &Rig) -> Option<State> {
    let s = strip(cx, rig);
    s.read_with(cx, |s, _| {
        s.settle();
        s.player_state()
    })
}

fn position(cx: &mut TestAppContext, rig: &Rig) -> u64 {
    let s = strip(cx, rig);
    s.read_with(cx, |s, _| s.position()).expect("a player")
}

/// Pull `frames` from the virtual device once the player's worker has caught up, then let
/// the window see it.
fn pull(cx: &mut TestAppContext, rig: &Rig, frames: usize) -> Vec<f32> {
    let s = strip(cx, rig);
    s.read_with(cx, |s, _| s.settle());
    let out = rig.script.audio().pull(frames);
    cx.run_until_parked();
    out
}

/// Let the analysis finish too, so the strip draws lanes; then draw.
fn settled(cx: &mut TestAppContext, rig: &Rig) {
    let s = strip(cx, rig);
    s.read_with(cx, |s, _| {
        s.wait();
        s.settle();
    });
    cx.run_until_parked();
    draw(cx, rig.window);
}

fn one_file(cx: &mut TestAppContext, seconds: u32) -> (Rig, String) {
    let rig = open_rig(cx);
    let a = rig.dir.write_wav("a.wav", 48_000, 2, 48_000 * seconds);
    add_paths(cx, &rig, &[&a]);
    settled(cx, &rig);
    (rig, a)
}

fn strip_bounds(cx: &mut TestAppContext, rig: &Rig, id: &'static str) -> Bounds<Pixels> {
    cx.update_window(rig.window, |_, window, _| window.find(id).bounds())
        .expect("the window is open")
}

fn frame_at(cx: &mut TestAppContext, rig: &Rig, x: f32) -> u64 {
    strip(cx, rig).read_with(cx, |s, _| s.view().frame_at(x))
}

#[gpui_test]
fn a_playable_file_gets_a_transport_stopped_at_the_start(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx, 2);
    assert!(has(cx, rig.window, "transport-play"));
    assert_eq!(state(cx, &rig), Some(State::Ready));
    let text = strip(cx, &rig).read_with(cx, |s, _| s.transport_text());
    assert_eq!(text.as_deref(), Some("0:00.0 / 0:02.0"));
    assert!(has(cx, rig.window, "transport-time"));
    assert!(!rig.script.audio().is_running(), "nothing plays by itself");
    // The loudness readout is still there beside it.
    let readout = strip(cx, &rig).read_with(cx, |s, _| s.readout_text());
    assert!(readout.unwrap().contains("LUFS"));
}

#[gpui_test]
fn the_button_plays_and_pauses(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx, 2);
    click(cx, rig.window, "transport-play");
    assert_eq!(state(cx, &rig), Some(State::Playing));
    assert!(rig.script.audio().is_running());
    assert_eq!(pull(cx, &rig, 4800).len(), 9600);
    draw(cx, rig.window);
    let text = strip(cx, &rig).read_with(cx, |s, _| s.transport_text());
    assert_eq!(text.as_deref(), Some("0:00.1 / 0:02.0"));
    click(cx, rig.window, "transport-play");
    assert_eq!(state(cx, &rig), Some(State::Paused));
    assert!(!rig.script.audio().is_running());
}

#[gpui_test]
fn space_toggles_from_the_files_list_and_from_the_strip(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx, 2);
    // The files list: the workspace holds focus after a row is clicked.
    click(cx, rig.window, "file-row-0");
    press(cx, rig.window, "space");
    assert_eq!(state(cx, &rig), Some(State::Playing));
    click(cx, rig.window, "waveform-strip");
    press(cx, rig.window, "space");
    assert_eq!(state(cx, &rig), Some(State::Paused));
}

#[gpui_test]
fn home_and_end_go_to_the_start_and_the_end(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx, 2);
    press(cx, rig.window, "space");
    pull(cx, &rig, 10_000);
    assert_eq!(position(cx, &rig), 10_000);
    press(cx, rig.window, "home");
    assert_eq!(position(cx, &rig), 0);
    press(cx, rig.window, "end");
    assert_eq!(position(cx, &rig), 96_000);
}

#[gpui_test]
fn a_space_typed_in_a_text_field_does_not_toggle(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx, 2);
    open_sheet(cx, &rig);
    click(cx, rig.window, "sheet-output");
    press(cx, rig.window, "space");
    assert_eq!(state(cx, &rig), Some(State::Ready));
}

#[gpui_test]
fn a_click_seeks_to_the_frame_under_the_pointer_at_any_zoom(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx, 4);
    let strip_entity = strip(cx, &rig);
    for zoom in 0..3 {
        cx.update_window(rig.window, |_, window, cx| {
            window.click_at("waveform-strip", point(px(120.), px(40.)), cx);
        })
        .unwrap();
        cx.run_until_parked();
        let want = frame_at(cx, &rig, 120.);
        assert!(want > 0, "zoom {zoom}");
        strip_entity.read_with(cx, |s, _| s.settle());
        assert_eq!(position(cx, &rig), want, "zoom {zoom}");
        press(cx, rig.window, "+");
        draw(cx, rig.window);
    }
}

#[gpui_test]
fn a_drag_scrubs_silently_and_the_release_seeks(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx, 4);
    click(cx, rig.window, "transport-play");
    pull(cx, &rig, 1000);
    let b = strip_bounds(cx, &rig, "waveform-strip");
    let y = b.origin.y + px(40.);
    let (from, to) = (b.origin.x + px(50.), b.origin.x + px(300.));
    cx.update_window(rig.window, |_, window, cx| {
        window.drag(point(from, y), point(to, y), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let want = frame_at(cx, &rig, 300.);
    assert_eq!(position(cx, &rig), want);
    let s = strip(cx, &rig);
    assert!(!s.read_with(cx, |s, _| s.is_scrubbing()));
    assert_eq!(state(cx, &rig), Some(State::Playing), "carries on after the drag");
}

#[gpui_test]
fn the_playhead_sits_at_the_position(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx, 4);
    click(cx, rig.window, "transport-play");
    pull(cx, &rig, 48_000);
    draw(cx, rig.window);
    let lanes = strip_bounds(cx, &rig, "waveform-strip");
    let head = strip_bounds(cx, &rig, "waveform-playhead");
    let want = strip(cx, &rig).read_with(cx, |s, _| s.view().x_of(48_000));
    let got = f32::from(head.origin.x - lanes.origin.x);
    assert!((got - want).abs() <= 1.0, "playhead at {got}, want {want}");
}

#[gpui_test]
fn the_view_follows_the_playhead_until_the_user_scrolls(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx, 8);
    let s = strip(cx, &rig);
    click(cx, rig.window, "waveform-strip");
    for _ in 0..3 {
        press(cx, rig.window, "+");
    }
    draw(cx, rig.window);
    let (_, end) = s.read_with(cx, |s, _| s.view().visible());
    press(cx, rig.window, "space");
    assert!(s.read_with(cx, |s, _| s.is_following()));
    pull(cx, &rig, end as usize + 100);
    draw(cx, rig.window);
    let (a, b) = s.read_with(cx, |s, _| s.view().visible());
    let at = position(cx, &rig);
    assert!(a <= at && at < b, "{at} in {a}..{b}");

    // A scroll of the user's own: the view stays where they put it.
    cx.update_window(rig.window, |_, window, cx| {
        window.scroll(
            "waveform-strip",
            ScrollDelta::Pixels(point(px(40.), px(0.))),
            cx,
        );
    })
    .unwrap();
    assert!(!s.read_with(cx, |s, _| s.is_following()));
    let start = s.read_with(cx, |s, _| s.view().start());
    pull(cx, &rig, 3 * (b - a) as usize);
    draw(cx, rig.window);
    assert_eq!(s.read_with(cx, |s, _| s.view().start()), start);

    // The next seek follows again.
    press(cx, rig.window, "home");
    draw(cx, rig.window);
    assert!(s.read_with(cx, |s, _| s.is_following()));
    assert_eq!(s.read_with(cx, |s, _| s.view().start()), 0);
}

#[gpui_test]
fn selecting_another_file_loads_it_stopped_at_zero(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let a = rig.dir.write_wav("a.wav", 48_000, 2, 96_000);
    let b = rig.dir.write_wav("b.wav", 44_100, 1, 44_100);
    add_paths(cx, &rig, &[&a, &b]);
    settled(cx, &rig);
    click(cx, rig.window, "file-row-0");
    settled(cx, &rig);
    press(cx, rig.window, "space");
    pull(cx, &rig, 5000);
    click(cx, rig.window, "file-row-1");
    settled(cx, &rig);
    assert_eq!(state(cx, &rig), Some(State::Ready));
    assert_eq!(position(cx, &rig), 0);
    assert!(!rig.script.audio().is_running());
    let text = strip(cx, &rig).read_with(cx, |s, _| s.transport_text());
    assert_eq!(text.as_deref(), Some("0:00.0 / 0:01.0"));
}

#[gpui_test]
fn a_file_that_cannot_be_read_gets_no_transport(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let junk = rig.dir.write("junk.wav", b"RIFF not really a wave file");
    add_paths(cx, &rig, &[&junk]);
    assert!(!has(cx, rig.window, "transport-play"));
    assert!(has(cx, rig.window, "inspector-error"));
}

#[gpui_test]
fn playback_works_while_the_waveform_is_still_analysing(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx, 1);
    let slow = rig.dir.write_wav("slow.wav", 48_000, 2, 48_000);
    let (hold, held) = mpsc::channel::<()>();
    let (reached, at_gate) = mpsc::channel::<()>();
    let s = strip(cx, &rig);
    s.update(cx, |s, cx| {
        s.select_with(None, true, cx, |_| unreachable!());
        s.select_with(Some(&slow), true, cx, |p| {
            let (held, reached) = (std::sync::Mutex::new(held), std::sync::Mutex::new(reached));
            Analyzer::start(p, Box::new(FsIo::new()), Limits::default(), move |snap| {
                if snap.state == AnalysisState::Running {
                    let _ = reached.lock().unwrap().send(());
                    let _ = held.lock().unwrap().recv();
                }
            })
        });
    });
    at_gate.recv().unwrap();
    cx.run_until_parked();
    draw(cx, rig.window);
    assert!(has(cx, rig.window, "transport-play"));
    click(cx, rig.window, "transport-play");
    assert_eq!(state(cx, &rig), Some(State::Playing));
    let got = pull(cx, &rig, 1000);
    assert!(got.iter().any(|s| *s != 0.0), "audio, not silence");
    drop(hold);
    settled(cx, &rig);
}

#[gpui_test]
fn with_no_audio_output_the_strip_says_so_and_the_rest_works(cx: &mut TestAppContext) {
    let (script, options) = crate::scripted::Script::options();
    let options = WorkspaceOptions {
        audio: AudioOutput::Off("No audio output".into()),
        ..options
    };
    let (window, workspace) = open_with(cx, options);
    let dir = crate::test_support::TempDir::new("no-audio");
    let a = dir.write_wav("a.wav", 48_000, 2, 48_000);
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |w, cx| {
            w.add_paths(vec![std::path::PathBuf::from(&a)], window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let s = workspace.read_with(cx, |w, _| w.strip().clone());
    s.read_with(cx, |s, _| s.wait());
    cx.run_until_parked();
    draw(cx, window);
    assert!(!has(cx, window, "transport-play"));
    assert!(has(cx, window, "transport-note"));
    assert!(has(cx, window, "waveform-strip"));
    assert_eq!(s.read_with(cx, |s, _| s.player_state()), None);
    drop(script);
}
