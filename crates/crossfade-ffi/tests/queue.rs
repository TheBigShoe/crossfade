//! The queue through the C functions: jobs run on the app's `ThreadRunner` and the caller
//! learns of it by asking and by waiting.
#![allow(clippy::undocumented_unsafe_blocks)]

mod common;

use common::{Dir, c, json_out, last_error, write_wav};
use crossfade_app::FsIo;
use crossfade_cli::{Outcome, run_command};
use crossfade_ffi::*;
use crossfade_graph::CancelToken;
use serde_json::{Value, json};

struct Queue(*mut CfQueue);

impl Queue {
    fn new() -> Self {
        let mut q = std::ptr::null_mut();
        assert_eq!(unsafe { cf_queue_new(&mut q) }, CF_OK);
        Self(q)
    }

    fn state(&self) -> Value {
        json_out(|out| unsafe { cf_queue_state(self.0, out) })
    }

    /// Queues what a sheet for `input` makes after `changes`.
    fn add(&self, input: &str, changes: Value) -> Result<u64, (i32, String)> {
        let path = c(input);
        let mut sheet = std::ptr::null_mut();
        assert_eq!(unsafe { cf_sheet_new(path.as_ptr(), &mut sheet) }, CF_OK);
        let changes = c(&changes.to_string());
        assert_eq!(unsafe { cf_sheet_apply(sheet, changes.as_ptr()) }, CF_OK);
        let mut id = u64::MAX;
        let status = unsafe { cf_queue_add(self.0, sheet, &mut id) };
        let message = last_error().unwrap_or_default();
        unsafe { cf_sheet_destroy(sheet) };
        if status == CF_OK {
            Ok(id)
        } else {
            Err((status, message))
        }
    }

    fn job(&self, id: u64) -> Value {
        let state = self.state();
        state["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"] == id)
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// Waits for changes until `done` holds for the job, returning every state of it seen.
    fn until(&self, id: u64, done: impl Fn(&Value) -> bool) -> Vec<Value> {
        let mut seen = Vec::new();
        let mut revision = 0;
        loop {
            let state = self.state();
            let job = state["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|j| j["id"] == id)
                .cloned()
                .unwrap();
            seen.push(job.clone());
            if done(&job) {
                return seen;
            }
            let at = state["revision"].as_u64().unwrap();
            assert_eq!(
                unsafe { cf_queue_wait(self.0, at, 60_000, &mut revision) },
                CF_OK
            );
            assert!(revision > at, "a minute passed with no change: {job}");
        }
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        unsafe { cf_queue_destroy(self.0) };
    }
}

fn is(state: &'static str) -> impl Fn(&Value) -> bool {
    move |job| job["state"] == state
}

#[test]
fn a_queued_job_writes_what_the_command_line_writes_with_progress_that_only_grows() {
    let dir = Dir::new("queue-run");
    let input = write_wav(&dir, "in.wav", 48_000, 2, 48_000 * 3);
    let out = dir.path("out.flac");
    let q = Queue::new();
    let id = q
        .add(&input, json!([{"output": out}, {"compression_level": 5}]))
        .unwrap();
    let seen = q.until(id, is("finished"));
    let last = seen.last().unwrap();
    assert_eq!(last["frames"], 48_000 * 3);
    assert_eq!(last["fraction"], 1.0);
    let done: Vec<u64> = seen
        .iter()
        .filter_map(|j| j["frames_done"].as_u64())
        .collect();
    assert!(done.windows(2).all(|w| w[0] <= w[1]), "{done:?}");
    assert_eq!(last["input"], input);
    assert_eq!(last["output"], out);
    assert!(
        last["command_line"]
            .as_str()
            .unwrap()
            .ends_with("--compression-level 5")
    );

    let cli_out = dir.path("cli.flac");
    let args = [
        "convert",
        &input,
        "-o",
        &cli_out,
        "--compression-level",
        "5",
    ];
    let outcome = run_command(args, &mut FsIo::new(), &CancelToken::new(), &mut |_| {}).unwrap();
    assert!(matches!(outcome, Outcome::Done(_)));
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(&cli_out).unwrap());
    let state = q.state();
    assert_eq!(state["idle"], true);
    assert_eq!(state["summary"]["finished"], 1);
}

#[test]
fn cancelling_a_running_job_leaves_no_partial_file() {
    let dir = Dir::new("queue-cancel");
    let input = write_wav(&dir, "long.wav", 48_000, 2, 48_000 * 20);
    let q = Queue::new();
    let id = q
        .add(
            &input,
            json!([{"output": dir.path("long.flac")}, {"compression_level": 8}]),
        )
        .unwrap();
    q.until(id, |j| j["frames_done"].as_u64().is_some_and(|d| d > 0));
    assert_eq!(unsafe { cf_queue_cancel(q.0, id) }, CF_OK);
    assert_eq!(q.job(id)["cancelling"], true);
    q.until(id, is("cancelled"));
    assert_eq!(dir.listing(), ["long.wav"]);
}

#[test]
fn a_waiting_job_cancels_at_once() {
    let dir = Dir::new("queue-cancel-pending");
    let input = write_wav(&dir, "in.wav", 48_000, 2, 48_000 * 10);
    let q = Queue::new();
    let first = q
        .add(&input, json!([{"output": dir.path("a.flac")}, {"compression_level": 8}]))
        .unwrap();
    let second = q.add(&input, json!({"output": dir.path("b.flac")})).unwrap();
    assert_eq!(q.job(second)["state"], "pending");
    assert_eq!(unsafe { cf_queue_cancel(q.0, second) }, CF_OK);
    assert_eq!(q.job(second)["state"], "cancelled");
    assert!(
        q.state()["quit_warning"]
            .as_str()
            .unwrap()
            .starts_with("1 conversion")
    );
    q.until(first, is("finished"));
    assert!(!dir.listing().contains(&"b.flac".to_owned()));
}

#[test]
fn a_failing_job_reports_its_message_and_a_retry_runs_it_again() {
    let dir = Dir::new("queue-fail");
    let input = write_wav(&dir, "in.wav", 48_000, 1, 4_800);
    let out = dir.path("later/out.wav");
    let q = Queue::new();
    let id = q.add(&input, json!({"output": out})).unwrap();
    let job = q.until(id, is("failed")).pop().unwrap();
    let message = job["message"].as_str().unwrap();
    assert!(message.contains("later"), "{message}");

    std::fs::create_dir(dir.path("later")).unwrap();
    let mut again = u64::MAX;
    assert_eq!(unsafe { cf_queue_retry(q.0, id, &mut again) }, CF_OK);
    assert_ne!(again, id);
    // The retry takes the failed row's place.
    assert_eq!(q.state()["jobs"].as_array().unwrap().len(), 1);
    q.until(again, is("finished"));
    assert!(std::path::Path::new(&out).exists());

    // Only a failed or cancelled job can be retried.
    let mut none = 0;
    assert_eq!(
        unsafe { cf_queue_retry(q.0, again, &mut none) },
        CF_ERR_REFUSED
    );
    assert_eq!(unsafe { cf_queue_retry(q.0, id, &mut none) }, CF_ERR_NOT_FOUND);
}

#[test]
fn remove_and_clear_take_rows_away_but_never_a_running_one() {
    let dir = Dir::new("queue-remove");
    let input = write_wav(&dir, "in.wav", 48_000, 2, 48_000 * 10);
    let q = Queue::new();
    let running = q
        .add(&input, json!([{"output": dir.path("a.flac")}, {"compression_level": 8}]))
        .unwrap();
    let waiting = q.add(&input, json!({"output": dir.path("b.wav")})).unwrap();
    assert_eq!(unsafe { cf_queue_remove(q.0, running) }, CF_ERR_REFUSED);
    assert!(last_error().is_some());
    assert_eq!(unsafe { cf_queue_remove(q.0, waiting) }, CF_OK);
    assert_eq!(q.job(waiting), Value::Null);
    assert_eq!(unsafe { cf_queue_remove(q.0, waiting) }, CF_ERR_NOT_FOUND);
    q.until(running, is("finished"));
    assert_eq!(unsafe { cf_queue_clear_done(q.0) }, CF_OK);
    assert_eq!(q.state()["jobs"], json!([]));
}

#[test]
fn a_sheet_with_problems_queues_nothing_and_says_why() {
    let dir = Dir::new("queue-refused");
    let input = write_wav(&dir, "in.wav", 48_000, 1, 480);
    std::fs::write(dir.path("taken.wav"), b"x").unwrap();
    let q = Queue::new();
    let (status, message) = q
        .add(&input, json!({"output": dir.path("taken.wav")}))
        .unwrap_err();
    assert_eq!(status, CF_ERR_REFUSED);
    assert_eq!(
        message,
        "That file already exists. Turn on replace to overwrite it."
    );
    assert_eq!(q.state()["jobs"], json!([]));
}

#[test]
fn destroying_the_queue_while_a_job_runs_stops_it_and_leaves_no_partial_file() {
    let dir = Dir::new("queue-destroy");
    let input = write_wav(&dir, "long.wav", 48_000, 2, 48_000 * 20);
    let q = Queue::new();
    let id = q
        .add(
            &input,
            json!([{"output": dir.path("long.flac")}, {"compression_level": 8}]),
        )
        .unwrap();
    q.until(id, |j| j["state"] == "running");
    drop(q);
    assert_eq!(dir.listing(), ["long.wav"]);
}

#[test]
fn waiting_with_no_change_returns_the_same_revision() {
    let q = Queue::new();
    let at = q.state()["revision"].as_u64().unwrap();
    let mut revision = u64::MAX;
    assert_eq!(unsafe { cf_queue_wait(q.0, at, 0, &mut revision) }, CF_OK);
    assert_eq!(revision, at);
    // A revision the caller has not seen yet returns at once.
    assert_eq!(
        unsafe { cf_queue_wait(q.0, at.wrapping_sub(1), 60_000, &mut revision) },
        CF_OK
    );
    assert_eq!(revision, at);
}

#[test]
fn null_handles_are_errors() {
    let mut out = std::ptr::null_mut();
    assert_eq!(
        unsafe { cf_queue_state(std::ptr::null(), &mut out) },
        CF_ERR_NULL_ARGUMENT
    );
    assert_eq!(unsafe { cf_queue_new(std::ptr::null_mut()) }, CF_ERR_NULL_ARGUMENT);
    let q = Queue::new();
    let mut id = 0;
    assert_eq!(
        unsafe { cf_queue_add(q.0, std::ptr::null(), &mut id) },
        CF_ERR_NULL_ARGUMENT
    );
    assert_eq!(
        unsafe { cf_queue_cancel(std::ptr::null(), 0) },
        CF_ERR_NULL_ARGUMENT
    );
    assert_eq!(unsafe { cf_queue_cancel(q.0, 99) }, CF_ERR_NOT_FOUND);
    unsafe { cf_queue_destroy(std::ptr::null_mut()) };
}
