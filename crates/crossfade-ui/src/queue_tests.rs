//! The queue dock: what a scripted runner's reports do to the rows, the status bar and the
//! dock itself.

use crossfade_app::Request;
use crossfade_app::queue::RunnerEvent;

use crate::prelude::*;
use crate::test_support::{Rig, click, draw, has, open_rig, step};
use crate::workspace::Workspace;

fn request(rig: &Rig, input: &str, output: &str) -> Request {
    let args = [
        "convert".to_owned(),
        rig.dir.path(input),
        "-o".to_owned(),
        rig.dir.path(output),
    ];
    crossfade_cli::request_from_args(args)
        .expect("valid arguments")
        .expect("a convert request")
}

fn enqueue(cx: &mut TestAppContext, rig: &Rig, input: &str, output: &str) {
    let request = request(rig, input, output);
    cx.update(|cx| {
        rig.workspace
            .update(cx, |workspace, cx| workspace.enqueue(request, cx));
    });
    cx.run_until_parked();
    draw(cx, rig.window);
}

fn send(cx: &mut TestAppContext, rig: &Rig, id: u64, event: RunnerEvent) {
    rig.script.send(usize::try_from(id).expect("small"), event);
    cx.run_until_parked();
    draw(cx, rig.window);
}

fn state(cx: &mut TestAppContext, workspace: &Entity<Workspace>, n: usize) -> String {
    workspace.read_with(cx, |w, _| w.queue_row_state(n))
}

fn rig_with_input(cx: &mut TestAppContext) -> Rig {
    let rig = open_rig(cx);
    rig.dir.write_wav("a.wav", 48_000, 2, 480);
    rig
}

#[gpui_test]
fn enqueuing_opens_the_dock_and_starts_the_first_job(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    draw(cx, rig.window);
    assert!(!has(cx, rig.window, "queue-dock"));
    assert!(!rig.workspace.read_with(cx, |w, _| w.queue_open()));

    enqueue(cx, &rig, "a.wav", "a-out.wav");
    assert!(rig.workspace.read_with(cx, |w, _| w.queue_open()));
    assert!(has(cx, rig.window, "queue-dock"));
    assert!(has(cx, rig.window, "queue-row-0"));
    assert_eq!(rig.script.starts().len(), 1);
    assert_eq!(
        rig.script.starts()[0].request,
        request(&rig, "a.wav", "a-out.wav")
    );
    // Started means running, with no total known yet.
    assert_eq!(state(cx, &rig.workspace, 0), "Converting\u{2026}");
}

#[gpui_test]
fn the_toolbar_toggle_shows_pressed_while_the_dock_is_open(cx: &mut TestAppContext) {
    let rig = open_rig(cx);
    draw(cx, rig.window);
    click(cx, rig.window, "toolbar-queue");
    draw(cx, rig.window);
    assert!(rig.workspace.read_with(cx, |w, _| w.queue_open()));
    assert!(has(cx, rig.window, "queue-dock"));
    let pressed = cx
        .update_window(rig.window, |_, window, _| {
            window.find("toolbar-queue").selected()
        })
        .expect("open");
    assert_eq!(pressed, Some(true));
    // The empty dock says so.
    assert!(has(cx, rig.window, "queue-empty"));

    press_toggle(cx, &rig);
    assert!(!has(cx, rig.window, "queue-dock"));
}

fn press_toggle(cx: &mut TestAppContext, rig: &Rig) {
    click(cx, rig.window, "toolbar-queue");
    draw(cx, rig.window);
}

#[gpui_test]
fn progress_updates_the_row_the_bar_and_the_status_text(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    enqueue(cx, &rig, "a.wav", "a-out.wav");
    send(
        cx,
        &rig,
        0,
        RunnerEvent::Progress {
            frames_done: 21,
            frames_total: Some(50),
        },
    );
    assert_eq!(state(cx, &rig.workspace, 0), "42%");
    assert!(has(cx, rig.window, "queue-progress-0"));
    assert_eq!(
        rig.workspace.read_with(cx, |w, _| w.status_text()),
        "Converting a-out.wav \u{2014} 42%"
    );
    assert_eq!(
        rig.workspace.read_with(cx, |w, _| w.status_queue()),
        "1 running"
    );
}

#[gpui_test]
fn cancel_asks_the_token_and_shows_cancelling_until_the_runner_reports(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    enqueue(cx, &rig, "a.wav", "a-out.wav");
    assert!(!rig.script.starts()[0].cancel.is_cancelled());

    click(cx, rig.window, "queue-cancel-0");
    draw(cx, rig.window);
    assert!(rig.script.starts()[0].cancel.is_cancelled());
    assert_eq!(state(cx, &rig.workspace, 0), "Cancelling\u{2026}");

    send(cx, &rig, 0, RunnerEvent::Cancelled);
    assert_eq!(state(cx, &rig.workspace, 0), "Cancelled");
    assert!(has(cx, rig.window, "queue-retry-0"));
    assert!(!has(cx, rig.window, "queue-cancel-0"));
}

#[gpui_test]
fn a_failure_shows_its_message_and_retry_queues_the_same_request_again(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    enqueue(cx, &rig, "a.wav", "a-out.wav");
    send(
        cx,
        &rig,
        0,
        RunnerEvent::Failed {
            message: "disk full".into(),
        },
    );
    assert_eq!(state(cx, &rig.workspace, 0), "Failed: disk full");

    click(cx, rig.window, "queue-retry-0");
    draw(cx, rig.window);
    assert!(has(cx, rig.window, "queue-row-1"));
    // The failed job is not running, so the retry starts at once.
    assert_eq!(rig.script.starts().len(), 2);
    assert_eq!(
        rig.script.starts()[1].request,
        rig.script.starts()[0].request
    );
    assert_eq!(
        rig.workspace.read_with(cx, |w, _| w.status_queue()),
        "1 running \u{b7} 1 failed"
    );
}

#[gpui_test]
fn finishing_then_clearing_empties_the_queue(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    enqueue(cx, &rig, "a.wav", "a-out.wav");
    send(cx, &rig, 0, RunnerEvent::Finished { frames: 6_000_000 });
    assert_eq!(state(cx, &rig.workspace, 0), "Done \u{b7} 6,000,000 frames");
    assert_eq!(
        rig.workspace.read_with(cx, |w, _| w.status_queue()),
        "1 done"
    );

    click(cx, rig.window, "queue-clear");
    draw(cx, rig.window);
    assert!(!has(cx, rig.window, "queue-row-0"));
    assert!(has(cx, rig.window, "queue-empty"));
    assert_eq!(rig.workspace.read_with(cx, |w, _| w.status_queue()), "");
}

#[gpui_test]
fn a_second_job_starts_only_after_the_first_ends(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    enqueue(cx, &rig, "a.wav", "one.wav");
    enqueue(cx, &rig, "a.wav", "two.wav");
    assert_eq!(rig.script.starts().len(), 1);
    assert_eq!(state(cx, &rig.workspace, 1), "Waiting");
    assert_eq!(
        rig.workspace.read_with(cx, |w, _| w.status_queue()),
        "1 running \u{b7} 1 waiting"
    );

    send(cx, &rig, 0, RunnerEvent::Finished { frames: 10 });
    assert_eq!(rig.script.starts().len(), 2);
    assert_eq!(state(cx, &rig.workspace, 1), "Converting\u{2026}");
}

#[gpui_test]
fn waiting_rows_can_be_cancelled_and_removed_but_running_ones_cannot_be_removed(
    cx: &mut TestAppContext,
) {
    let rig = rig_with_input(cx);
    enqueue(cx, &rig, "a.wav", "one.wav");
    enqueue(cx, &rig, "a.wav", "two.wav");
    assert!(has(cx, rig.window, "queue-cancel-0"));
    assert!(!has(cx, rig.window, "queue-remove-0"));
    assert!(has(cx, rig.window, "queue-remove-1"));

    click(cx, rig.window, "queue-cancel-1");
    draw(cx, rig.window);
    assert_eq!(state(cx, &rig.workspace, 1), "Cancelled");
    click(cx, rig.window, "queue-remove-1");
    draw(cx, rig.window);
    assert!(!has(cx, rig.window, "queue-row-1"));
}

#[gpui_test]
fn copy_as_command_puts_the_command_line_on_the_clipboard(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    enqueue(cx, &rig, "a.wav", "a-out.wav");
    let expected = rig
        .workspace
        .read_with(cx, |w, _| w.queue().command_line(rig.script.starts()[0].id))
        .expect("expressible");

    click(cx, rig.window, "queue-more-0");
    step(cx, 2);
    draw(cx, rig.window);
    cx.update_window(rig.window, |_, window, cx| {
        window.within("popup-menu").click(0usize, cx);
    })
    .expect("open");
    cx.run_until_parked();
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(copied, Some(expected));
}

#[gpui_test]
fn show_in_folder_is_offered_only_for_finished_jobs(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    enqueue(cx, &rig, "a.wav", "a-out.wav");
    assert_eq!(
        rig.workspace.read_with(cx, |w, _| w.queue_menu_labels(0)),
        ["Copy as command"]
    );
    send(cx, &rig, 0, RunnerEvent::Finished { frames: 10 });
    assert_eq!(
        rig.workspace.read_with(cx, |w, _| w.queue_menu_labels(0)),
        ["Copy as command", "Show in folder"]
    );
}

#[gpui_test]
fn the_status_counts_name_each_non_zero_state(cx: &mut TestAppContext) {
    let rig = rig_with_input(cx);
    for out in ["1.wav", "2.wav", "3.wav", "4.wav"] {
        enqueue(cx, &rig, "a.wav", out);
    }
    send(cx, &rig, 0, RunnerEvent::Finished { frames: 1 });
    send(
        cx,
        &rig,
        1,
        RunnerEvent::Failed {
            message: "x".into(),
        },
    );
    assert_eq!(
        rig.workspace.read_with(cx, |w, _| w.status_queue()),
        "1 running \u{b7} 1 waiting \u{b7} 1 failed \u{b7} 1 done"
    );
}
