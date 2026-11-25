//! The waveform strip in the inspector: it renders the `crossfade-app` strip model, so these
//! check what is drawn and that each gesture reaches the model.

use std::sync::mpsc;

use crossfade_app::FsIo;
use crossfade_app::analysis::{Analyzer, State};
use crossfade_core::Limits;
use crossfade_graph::CancelToken;

use crate::actions::{WaveformFit, WaveformPanRight, WaveformZoomIn};
use crate::config_test_support::TempDir as ConfigDir;
use crate::prelude::*;
use crate::test_support::{Rig, add_paths, click, draw, has, open_rig, open_rig_in, press};
use crate::waveform_view::WaveformStrip;

fn strip(cx: &mut TestAppContext, rig: &Rig) -> Entity<WaveformStrip> {
    rig.workspace.read_with(cx, |w, _| w.strip().clone())
}

/// Wait for the shown file's analysis to end, deliver its wake-up and draw.
fn finish(cx: &mut TestAppContext, rig: &Rig) {
    let strip = strip(cx, rig);
    strip.read_with(cx, |s, _| s.wait());
    cx.run_until_parked();
    draw(cx, rig.window);
}

fn fpp(cx: &mut TestAppContext, strip: &Entity<WaveformStrip>) -> u64 {
    strip.read_with(cx, |s, _| s.view().frames_per_pixel())
}

fn readout(cx: &mut TestAppContext, rig: &Rig) -> Option<String> {
    strip(cx, rig).read_with(cx, |s, _| s.readout_text())
}

/// Replace the shown file's analysis with `analyzer` (selecting nothing in between).
fn install(
    cx: &mut TestAppContext,
    rig: &Rig,
    path: &str,
    analyzer: impl FnOnce(&str) -> Analyzer,
) {
    let strip = strip(cx, rig);
    strip.update(cx, |s, cx| {
        s.select_with(None, true, cx, |_| unreachable!());
        s.select_with(Some(path), true, cx, analyzer);
    });
    cx.run_until_parked();
    draw(cx, rig.window);
}

fn one_file(cx: &mut TestAppContext) -> (Rig, String) {
    let rig = open_rig(cx);
    let a = rig.dir.write_wav("a.wav", 48_000, 2, 48_000);
    add_paths(cx, &rig, &[&a]);
    (rig, a)
}

#[gpui_test]
fn the_strip_ruler_and_readout_appear_for_an_analysed_file(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx);
    finish(cx, &rig);
    for id in ["waveform-strip", "waveform-ruler", "waveform-readout"] {
        assert!(has(cx, rig.window, id), "{id}");
    }
    assert!(!has(cx, rig.window, "waveform-analyse"));
    let text = readout(cx, &rig).expect("a readout");
    assert!(text.contains("LUFS") && text.contains("dBTP"), "{text}");
}

#[gpui_test]
fn a_partial_analysis_says_analysing_and_still_draws(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx);
    let slow = rig.dir.write_wav("slow.wav", 48_000, 2, 48_000);
    let (hold, held) = mpsc::channel::<()>();
    let (reached, at_gate) = mpsc::channel::<()>();
    install(cx, &rig, &slow, |p| {
        let (held, reached) = (std::sync::Mutex::new(held), std::sync::Mutex::new(reached));
        Analyzer::start(p, Box::new(FsIo::new()), Limits::default(), move |snap| {
            if snap.state == State::Running {
                let _ = reached.lock().unwrap().send(());
                let _ = held.lock().unwrap().recv();
            }
        })
    });
    at_gate.recv().unwrap();
    cx.run_until_parked();
    draw(cx, rig.window);
    let text = readout(cx, &rig).expect("a readout");
    assert!(text.starts_with("Analysing\u{2026}"), "{text}");
    assert!(has(cx, rig.window, "waveform-strip"));
    drop(hold);
    finish(cx, &rig);
    assert!(readout(cx, &rig).unwrap().contains("LUFS"));
}

#[gpui_test]
fn a_failed_analysis_is_one_muted_line_and_the_inspector_stays(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx);
    let junk = rig.dir.write("junk.wav", b"not audio at all");
    install(cx, &rig, &junk, |p| {
        let a = Analyzer::start(p, Box::new(FsIo::new()), Limits::default(), |_| {});
        assert!(matches!(a.wait().state, State::Failed(_)));
        a
    });
    let text = readout(cx, &rig).expect("a line");
    assert!(text.starts_with("Could not analyse: "), "{text}");
    assert!(has(cx, rig.window, "waveform-error"));
    assert!(!has(cx, rig.window, "waveform-strip"));
    assert!(has(cx, rig.window, "inspector-path"));
}

#[gpui_test]
fn changing_the_selection_cancels_a_running_analysis_and_switches(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let a = rig.dir.write_wav("a.wav", 48_000, 2, 48_000);
    let b = rig.dir.write_wav("b.wav", 44_100, 1, 44_100);
    add_paths(cx, &rig, &[&a, &b]);
    finish(cx, &rig);
    let token = CancelToken::new();
    let (hold, held) = mpsc::channel::<()>();
    let (reached, at_gate) = mpsc::channel::<()>();
    let t = token.clone();
    let c = rig.dir.write_wav("c.wav", 48_000, 2, 48_000);
    install(cx, &rig, &c, move |p| {
        let (held, reached) = (std::sync::Mutex::new(held), std::sync::Mutex::new(reached));
        Analyzer::start_with_cancel(
            p,
            Box::new(FsIo::new()),
            Limits::default(),
            t,
            move |snap| {
                if snap.state == State::Running {
                    let _ = reached.lock().unwrap().send(());
                    let _ = held.lock().unwrap().recv();
                }
            },
        )
    });
    at_gate.recv().unwrap();
    click(cx, rig.window, "file-row-0");
    draw(cx, rig.window);
    assert!(token.is_cancelled());
    let shown = strip(cx, &rig).read_with(cx, |s, _| s.path().map(str::to_owned));
    assert_eq!(shown.as_deref(), Some(a.as_str()));
    drop(hold);
    finish(cx, &rig);
    assert!(readout(cx, &rig).unwrap().contains("LUFS"));
}

#[gpui_test]
fn the_keys_zoom_pan_and_fit_the_focused_strip(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx);
    finish(cx, &rig);
    let strip = strip(cx, &rig);
    assert!(strip.read_with(cx, |s, _| s.view().width()) > 0);
    let fit = fpp(cx, &strip);
    click(cx, rig.window, "waveform-strip");
    press(cx, rig.window, "+");
    let zoomed = fpp(cx, &strip);
    assert!(zoomed < fit);
    press(cx, rig.window, "=");
    assert!(fpp(cx, &strip) < zoomed);
    let closer = fpp(cx, &strip);
    press(cx, rig.window, "-");
    assert!(fpp(cx, &strip) > closer);
    press(cx, rig.window, "right");
    let start = strip.read_with(cx, |s, _| s.view().start());
    assert!(start > 0);
    press(cx, rig.window, "left");
    assert!(strip.read_with(cx, |s, _| s.view().start()) < start);
    press(cx, rig.window, "0");
    assert_eq!(fpp(cx, &strip), fit);
    draw(cx, rig.window);
    assert!(!has(cx, rig.window, "waveform-position"));
    press(cx, rig.window, "+");
    draw(cx, rig.window);
    assert!(has(cx, rig.window, "waveform-position"));
}

#[gpui_test]
fn keys_do_nothing_unless_the_strip_has_focus(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx);
    finish(cx, &rig);
    let strip = strip(cx, &rig);
    let fit = fpp(cx, &strip);
    press(cx, rig.window, "+");
    assert_eq!(fpp(cx, &strip), fit);
}

#[gpui_test]
fn double_click_fits_the_whole_file(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx);
    finish(cx, &rig);
    let strip = strip(cx, &rig);
    let fit = fpp(cx, &strip);
    click(cx, rig.window, "waveform-strip");
    press(cx, rig.window, "+");
    assert!(fpp(cx, &strip) < fit);
    cx.update_window(rig.window, |_, window, cx| {
        window.double_click("waveform-strip", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(fpp(cx, &strip), fit);
}

/// One frame as the platform draws it: no refresh first, so a cached view is reused unless it
/// was notified before the frame began.
fn one_frame(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    cx.run_until_parked();
}

fn lanes_show_the_view(cx: &mut TestAppContext, strip: &Entity<WaveformStrip>) -> bool {
    strip.read_with(cx, |s, cx| {
        let v = s.view();
        s.lanes_painted(cx) == Some((v.start(), v.frames_per_pixel(), v.width()))
    })
}

#[gpui_test]
fn the_frame_after_a_zoom_pan_or_fit_paints_the_new_range(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx);
    finish(cx, &rig);
    let strip = strip(cx, &rig);
    click(cx, rig.window, "waveform-strip");
    // Dispatched straight to the focus: `press` draws a refreshed frame of its own afterwards.
    let actions: [Box<dyn Action>; 3] = [
        Box::new(WaveformZoomIn),
        Box::new(WaveformPanRight),
        Box::new(WaveformFit),
    ];
    for action in actions {
        let name = action.name();
        cx.update_window(rig.window, |_, window, cx| {
            window.dispatch_action(action, cx)
        })
        .unwrap();
        cx.run_until_parked();
        one_frame(cx, rig.window);
        assert!(lanes_show_the_view(cx, &strip), "after {name}");
    }
}

#[gpui_test]
fn a_resize_asks_for_the_frame_that_paints_the_new_width(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx);
    finish(cx, &rig);
    let strip = strip(cx, &rig);
    let before = strip.read_with(cx, |s, _| s.view().width());
    VisualTestContext::from_window(rig.window, cx).simulate_resize(size(px(800.), px(500.)));
    // The resize's own frame lays the strip out and only then learns its new width.
    one_frame(cx, rig.window);
    assert_ne!(strip.read_with(cx, |s, _| s.view().width()), before);
    let asked = cx
        .update_window(rig.window, |_, window, cx| window.simulate_next_frame(cx))
        .unwrap();
    assert!(asked > 0, "no frame was asked for after the width changed");
    cx.run_until_parked();
    one_frame(cx, rig.window);
    assert!(lanes_show_the_view(cx, &strip));
}

fn secondary() -> Modifiers {
    Modifiers {
        platform: cfg!(target_os = "macos"),
        control: !cfg!(target_os = "macos"),
        ..Modifiers::default()
    }
}

#[gpui_test]
fn scroll_pans_and_secondary_scroll_and_pinch_zoom(cx: &mut TestAppContext) {
    let (rig, _) = one_file(cx);
    finish(cx, &rig);
    let strip = strip(cx, &rig);
    click(cx, rig.window, "waveform-strip");
    press(cx, rig.window, "+");
    press(cx, rig.window, "+");
    let zoomed = fpp(cx, &strip);

    cx.update_window(rig.window, |_, window, cx| {
        window.scroll(
            "waveform-strip",
            ScrollDelta::Pixels(point(px(-40.), px(0.))),
            cx,
        );
    })
    .unwrap();
    assert!(
        strip.read_with(cx, |s, _| s.view().start()) > 0,
        "panned later"
    );
    assert_eq!(fpp(cx, &strip), zoomed);

    cx.update_window(rig.window, |_, window, cx| {
        window.hover("waveform-strip", cx);
        let at = window.mouse_position();
        window.dispatch_event(
            ScrollEvent {
                position: at,
                delta: ScrollDelta::Pixels(point(px(0.), px(60.))),
                modifiers: secondary(),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    })
    .unwrap();
    let in_by_scroll = fpp(cx, &strip);
    assert!(in_by_scroll < zoomed);

    cx.update_window(rig.window, |_, window, cx| {
        let at = window.mouse_position();
        window.dispatch_event(
            PinchEvent {
                position: at,
                delta: -0.5,
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    })
    .unwrap();
    assert!(fpp(cx, &strip) > in_by_scroll, "pinching out zooms out");
}

#[gpui_test]
fn with_automatic_analysis_off_the_strip_offers_analyse(cx: &mut TestAppContext) {
    let config = ConfigDir::new("analyse-off");
    config.write("settings.json", r#"{ "analyze_on_select": false }"#);
    let rig = open_rig_in(cx, config);
    let a = rig.dir.write_wav("a.wav", 48_000, 2, 48_000);
    add_paths(cx, &rig, &[&a]);
    assert!(has(cx, rig.window, "waveform-analyse"));
    assert!(!has(cx, rig.window, "waveform-strip"));
    assert!(!strip(cx, &rig).read_with(cx, |s, _| s.has_analysis()));
    click(cx, rig.window, "waveform-analyse");
    finish(cx, &rig);
    assert!(has(cx, rig.window, "waveform-strip"));
    assert!(!has(cx, rig.window, "waveform-analyse"));
}

#[gpui_test]
fn a_file_with_no_audio_stream_gets_no_strip(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let junk = rig.dir.write("junk.wav", b"RIFF not really a wave file");
    add_paths(cx, &rig, &[&junk]);
    assert!(!has(cx, rig.window, "waveform-strip"));
    assert!(!has(cx, rig.window, "waveform-analyse"));
}
