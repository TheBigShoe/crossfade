//! Quitting with conversions running: the app asks first, then cancels them and leaves
//! only once the worker has reported the end. The test platform's `quit` does nothing, so
//! the workspace's quit step is a seam these tests count calls to.

use std::cell::Cell;
use std::rc::Rc;

use crossfade_app::Request;
use crossfade_app::queue::RunnerEvent;

use crate::actions::Quit;
use crate::config::{self, ConfigPiece};
use crate::config_test_support::TempDir as ConfigDir;
use crate::prelude::*;
use crate::test_support::{Rig, click, draw, has, open_rig, open_rig_in, press, step};

fn request(rig: &Rig, name: &str) -> Request {
    let args = [
        "convert".to_owned(),
        rig.dir.path(&format!("{name}.wav")),
        "-o".to_owned(),
        rig.dir.path(&format!("{name}-out.wav")),
    ];
    crossfade_cli::request_from_args(args)
        .expect("valid arguments")
        .expect("a convert request")
}

fn enqueue(cx: &mut TestAppContext, rig: &Rig, name: &str) {
    let request = request(rig, name);
    cx.update(|cx| {
        rig.workspace
            .update(cx, |workspace, cx| workspace.enqueue(request, cx));
    });
    cx.run_until_parked();
}

/// Counts the times the workspace actually asks the app to quit.
fn count_quits(cx: &mut TestAppContext, rig: &Rig) -> Rc<Cell<u32>> {
    let quits = Rc::new(Cell::new(0));
    let seen = Rc::clone(&quits);
    cx.update(|cx| {
        rig.workspace.update(cx, |workspace, _| {
            workspace.set_quit(Rc::new(move |_| seen.set(seen.get() + 1)));
        });
    });
    quits
}

/// Ask to quit and let the confirmation, if there is one, open.
fn quit(cx: &mut TestAppContext, rig: &Rig) {
    cx.dispatch_action(rig.window, Quit);
    cx.run_until_parked();
    step(cx, 3);
    draw(cx, rig.window);
}

/// Ask to quit and answer the confirmation with "Quit".
fn quit_for_sure(cx: &mut TestAppContext, rig: &Rig) {
    quit(cx, rig);
    assert!(has(cx, rig.window, "quit-confirm"), "the dialog is open");
    click(cx, rig.window, "dialog-ok");
}

fn asking(cx: &mut TestAppContext, rig: &Rig) -> bool {
    has(cx, rig.window, "quit-confirm")
}

fn cancelled(rig: &Rig, at: usize) -> bool {
    rig.script.starts()[at].cancel.is_cancelled()
}

#[gpui_test]
fn quitting_with_nothing_running_quits_at_once(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    quit(cx, &rig);
    assert!(!asking(cx, &rig));
    assert_eq!(quits.get(), 1);
}

#[gpui_test]
fn quitting_with_a_running_job_asks_and_cancels_nothing(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    enqueue(cx, &rig, "a");
    quit(cx, &rig);
    assert!(asking(cx, &rig));
    assert!(!cancelled(&rig, 0));
    assert_eq!(quits.get(), 0);
    let body = rig.workspace.read_with(cx, |w, _| w.quit_prompt());
    assert_eq!(
        body.as_deref(),
        Some("1 conversion is still running. Quitting will cancel it.")
    );
}

#[gpui_test]
fn the_dialog_counts_waiting_jobs_too(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    enqueue(cx, &rig, "a");
    enqueue(cx, &rig, "b");
    enqueue(cx, &rig, "c");
    quit(cx, &rig);
    let body = rig.workspace.read_with(cx, |w, _| w.quit_prompt());
    assert_eq!(
        body.as_deref(),
        Some("3 conversions are still running or waiting. Quitting will cancel them.")
    );
}

#[gpui_test]
fn keep_converting_closes_the_dialog_and_changes_nothing(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    enqueue(cx, &rig, "a");
    quit(cx, &rig);
    click(cx, rig.window, "dialog-cancel");
    step(cx, 3);
    draw(cx, rig.window);
    assert!(!asking(cx, &rig));
    assert!(!cancelled(&rig, 0));
    rig.script.send(0, RunnerEvent::Finished { frames: 1 });
    cx.run_until_parked();
    assert_eq!(quits.get(), 0, "a later completion does not quit either");
}

#[gpui_test]
fn escape_keeps_converting_too(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    enqueue(cx, &rig, "a");
    quit(cx, &rig);
    press(cx, rig.window, "escape");
    step(cx, 3);
    draw(cx, rig.window);
    assert!(!asking(cx, &rig));
    assert!(!cancelled(&rig, 0));
    assert_eq!(quits.get(), 0);
    // The dialog can be asked for again afterwards.
    quit(cx, &rig);
    assert!(asking(cx, &rig));
}

#[gpui_test]
fn quit_in_the_dialog_cancels_the_job_and_waits_for_the_runner_to_say_so(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    enqueue(cx, &rig, "a");
    enqueue(cx, &rig, "b");
    assert!(!cancelled(&rig, 0));

    quit_for_sure(cx, &rig);
    assert!(cancelled(&rig, 0));
    assert_eq!(quits.get(), 0, "the worker has not reported yet");
    assert_eq!(rig.script.starts().len(), 1, "the waiting job never starts");

    rig.script.send(0, RunnerEvent::Cancelled);
    cx.run_until_parked();
    assert_eq!(quits.get(), 1);
    draw(cx, rig.window);
}

#[gpui_test]
fn a_job_that_finishes_before_it_sees_the_cancel_also_lets_the_app_quit(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    enqueue(cx, &rig, "a");
    quit_for_sure(cx, &rig);
    rig.script.send(0, RunnerEvent::Finished { frames: 1 });
    cx.run_until_parked();
    assert_eq!(quits.get(), 1);
}

#[gpui_test]
fn asking_twice_while_waiting_quits_once(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    enqueue(cx, &rig, "a");
    quit_for_sure(cx, &rig);
    quit(cx, &rig);
    assert!(!asking(cx, &rig), "no second dialog while leaving");
    rig.script.send(0, RunnerEvent::Cancelled);
    cx.run_until_parked();
    assert_eq!(quits.get(), 1);
}

#[gpui_test]
fn the_wait_is_bounded_when_the_worker_never_answers(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    enqueue(cx, &rig, "a");
    quit_for_sure(cx, &rig);
    assert_eq!(quits.get(), 0);
    cx.executor()
        .advance_clock(crate::workspace::QUIT_GRACE + std::time::Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(quits.get(), 1);
}

#[gpui_test]
fn a_second_quit_while_the_dialog_is_open_opens_no_second_one(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    enqueue(cx, &rig, "a");
    quit(cx, &rig);
    quit(cx, &rig);
    // One answer closes it: nothing is stacked underneath.
    click(cx, rig.window, "dialog-cancel");
    step(cx, 3);
    draw(cx, rig.window);
    assert!(!asking(cx, &rig));
    assert_eq!(quits.get(), 0);
}

#[gpui_test]
fn the_dialog_closes_by_itself_when_the_queue_goes_idle_and_the_app_stays(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    enqueue(cx, &rig, "a");
    quit(cx, &rig);
    rig.script.send(0, RunnerEvent::Finished { frames: 1 });
    cx.run_until_parked();
    step(cx, 3);
    draw(cx, rig.window);
    assert!(!asking(cx, &rig));
    assert_eq!(quits.get(), 0, "the user never confirmed");
}

#[gpui_test]
fn the_dialog_opens_over_the_convert_sheet(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    let input = rig.dir.write_wav("in.wav", 48_000, 2, 100);
    crate::test_support::add_paths(cx, &rig, &[&input]);
    enqueue(cx, &rig, "a");
    crate::test_support::open_sheet(cx, &rig);
    assert!(has(cx, rig.window, "dialog-ok"));
    quit(cx, &rig);
    assert!(asking(cx, &rig));
    // Keeping the work leaves the sheet where it was; both footers carry the same ids, so
    // the click names its dialog.
    cx.update_window(rig.window, |_, window, cx| {
        window.within("quit-confirm").click("dialog-cancel", cx);
    })
    .expect("open");
    cx.run_until_parked();
    step(cx, 3);
    draw(cx, rig.window);
    assert!(!asking(cx, &rig));
    assert!(
        rig.workspace
            .read_with(cx, |w, _| w.convert_dialog().is_some())
    );
    assert!(
        has(cx, rig.window, "sheet-output"),
        "the sheet is still drawn"
    );
    assert_eq!(quits.get(), 0);
}

#[gpui_test]
fn closing_the_window_mid_conversion_asks_the_same_question(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    enqueue(cx, &rig, "a");
    let mut visual = VisualTestContext::from_window(rig.window, cx);
    assert!(
        !visual.simulate_close(),
        "the window stays until the job ends"
    );
    cx.run_until_parked();
    step(cx, 3);
    draw(cx, rig.window);
    assert!(asking(cx, &rig));
    assert!(!cancelled(&rig, 0));
    click(cx, rig.window, "dialog-ok");
    assert!(cancelled(&rig, 0));
    rig.script.send(0, RunnerEvent::Cancelled);
    cx.run_until_parked();
    assert_eq!(quits.get(), 1);
}

#[gpui_test]
fn closing_an_idle_window_just_closes(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    let mut visual = VisualTestContext::from_window(rig.window, cx);
    assert!(visual.simulate_close());
    assert_eq!(
        quits.get(),
        0,
        "the app quits on the window-closed event instead"
    );
}

#[gpui_test]
fn with_confirm_quit_off_a_running_job_is_cancelled_and_the_app_quits_unasked(
    cx: &mut TestAppContext,
) {
    let config = ConfigDir::new("no-confirm");
    config.write("settings.json", r#"{ "confirm_quit": false }"#);
    let rig = open_rig_in(cx, config);
    let quits = count_quits(cx, &rig);
    enqueue(cx, &rig, "a");
    quit(cx, &rig);
    assert!(!asking(cx, &rig));
    assert!(cancelled(&rig, 0));
    rig.script.send(0, RunnerEvent::Cancelled);
    cx.run_until_parked();
    assert_eq!(quits.get(), 1);
}

#[gpui_test]
fn confirm_quit_reloads_live(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    enqueue(cx, &rig, "a");
    rig.config
        .write("settings.json", r#"{ "confirm_quit": false }"#);
    cx.update(|cx| config::reload(ConfigPiece::Settings, cx));
    quit(cx, &rig);
    assert!(!asking(cx, &rig));
    assert!(cancelled(&rig, 0));
    assert_eq!(quits.get(), 0, "still waiting for the runner");
}
