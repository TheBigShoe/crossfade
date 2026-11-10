//! Quitting with conversions running: the app cancels them and leaves only once the
//! worker has reported the end. The test platform's `quit` does nothing, so the
//! workspace's quit step is a seam these tests count calls to.

use std::cell::Cell;
use std::rc::Rc;

use crossfade_app::Request;
use crossfade_app::queue::RunnerEvent;

use crate::actions::Quit;
use crate::prelude::*;
use crate::test_support::{Rig, draw, open_rig};

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

fn quit(cx: &mut TestAppContext, rig: &Rig) {
    cx.dispatch_action(rig.window, Quit);
    cx.run_until_parked();
}

#[gpui_test]
fn quitting_with_nothing_running_quits_at_once(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    quit(cx, &rig);
    assert_eq!(quits.get(), 1);
}

#[gpui_test]
fn quitting_cancels_the_job_and_waits_for_the_runner_to_say_so(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    enqueue(cx, &rig, "a");
    enqueue(cx, &rig, "b");
    assert!(!rig.script.starts()[0].cancel.is_cancelled());

    quit(cx, &rig);
    assert!(rig.script.starts()[0].cancel.is_cancelled());
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
    quit(cx, &rig);
    rig.script.send(0, RunnerEvent::Finished { frames: 1 });
    cx.run_until_parked();
    assert_eq!(quits.get(), 1);
}

#[gpui_test]
fn asking_twice_while_waiting_quits_once(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    enqueue(cx, &rig, "a");
    quit(cx, &rig);
    quit(cx, &rig);
    rig.script.send(0, RunnerEvent::Cancelled);
    cx.run_until_parked();
    assert_eq!(quits.get(), 1);
}

#[gpui_test]
fn the_wait_is_bounded_when_the_worker_never_answers(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    enqueue(cx, &rig, "a");
    quit(cx, &rig);
    assert_eq!(quits.get(), 0);
    cx.executor()
        .advance_clock(crate::workspace::QUIT_GRACE + std::time::Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(quits.get(), 1);
}

#[gpui_test]
fn closing_the_window_mid_conversion_is_a_quit_that_waits(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    let quits = count_quits(cx, &rig);
    enqueue(cx, &rig, "a");
    let mut visual = VisualTestContext::from_window(rig.window, cx);
    assert!(
        !visual.simulate_close(),
        "the window stays until the job ends"
    );
    assert!(rig.script.starts()[0].cancel.is_cancelled());
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
