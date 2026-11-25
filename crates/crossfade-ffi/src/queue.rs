//! The conversion queue across the boundary: the app's `Queue` pumped by its `ThreadRunner`.
//! The runner's reports are applied on the worker thread as they come, so the queue's state
//! is always current when asked for, and the revision says when it changed.

use crate::error::{CF_ERR_NOT_FOUND, Failure};
use crate::sheet::Sheet;
use crate::signal::Signal;
use crossfade_app::command_line;
use crossfade_app::queue::{JobId, JobState, Queue, QueueItem, quit_warning};
use crossfade_app::runner::ThreadRunner;
use crossfade_core::Limits;
use serde::Serialize;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

struct Inner {
    queue: Queue,
    runner: ThreadRunner,
}

struct Shared {
    inner: Mutex<Inner>,
    signal: Signal,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs `f` on the queue, starts the next job if one may start, and says it changed.
    fn change<R>(&self, f: impl FnOnce(&mut Queue) -> R) -> R {
        let mut guard = self.lock();
        let Inner { queue, runner } = &mut *guard;
        let out = f(queue);
        queue.pump(runner);
        drop(guard);
        self.signal.bump();
        out
    }
}

/// A queue any thread may use. Dropping it cancels every job and waits for the running one to
/// stop, so no partial file is left behind.
pub struct JobQueue {
    shared: Arc<Shared>,
}

fn not_found() -> Failure {
    Failure::new(CF_ERR_NOT_FOUND, "There is no conversion with that id.")
}

#[derive(Serialize)]
struct NormalizationState {
    measured_lufs: Option<f64>,
    target_lufs: f64,
    reached_lufs: Option<f64>,
    gain_db: f64,
    limited_by_ceiling: bool,
    ceiling_dbtp: f64,
}

#[derive(Serialize)]
struct Job {
    id: u64,
    input: String,
    output: String,
    command_line: Option<String>,
    /// `pending`, `running`, `finished`, `failed` or `cancelled`.
    state: &'static str,
    frames_done: Option<u64>,
    frames_total: Option<u64>,
    fraction: Option<f32>,
    cancelling: bool,
    frames: Option<u64>,
    message: Option<String>,
    measuring: bool,
    normalization: Option<NormalizationState>,
    clipped: u64,
}

#[derive(Serialize)]
struct Summary {
    pending: usize,
    running: usize,
    finished: usize,
    failed: usize,
    cancelled: usize,
}

#[derive(Serialize)]
struct State {
    revision: u64,
    idle: bool,
    summary: Summary,
    quit_warning: Option<String>,
    jobs: Vec<Job>,
}

fn job(queue: &Queue, item: &QueueItem) -> Job {
    let first = |paths: Vec<String>| paths.into_iter().next().unwrap_or_default();
    let (state, frames_done, frames_total, cancelling, frames, message) = match &item.state {
        JobState::Pending => ("pending", None, None, false, None, None),
        JobState::Running {
            frames_done,
            frames_total,
            cancelling,
        } => (
            "running",
            Some(*frames_done),
            *frames_total,
            *cancelling,
            None,
            None,
        ),
        JobState::Finished { frames } => ("finished", None, None, false, Some(*frames), None),
        JobState::Failed { message } => ("failed", None, None, false, None, Some(message.clone())),
        JobState::Cancelled => ("cancelled", None, None, false, None, None),
    };
    Job {
        id: item.id.value(),
        input: first(
            item.request
                .job
                .inputs
                .iter()
                .map(|i| i.path.clone())
                .collect(),
        ),
        output: first(
            item.request
                .job
                .outputs
                .iter()
                .map(|o| o.path.clone())
                .collect(),
        ),
        command_line: command_line(&item.request).ok(),
        state,
        frames_done,
        frames_total,
        fraction: queue.fraction(item.id),
        cancelling,
        frames,
        message,
        measuring: item.measuring,
        normalization: item.normalization.map(|n| NormalizationState {
            measured_lufs: n.measured_lufs,
            target_lufs: n.target_lufs,
            reached_lufs: n.reached_lufs(),
            gain_db: n.gain_db,
            limited_by_ceiling: n.limited_by_ceiling,
            ceiling_dbtp: n.ceiling_dbtp,
        }),
        clipped: item.clipped,
    }
}

impl Default for JobQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl JobQueue {
    pub fn new() -> Self {
        let shared = Arc::new_cyclic(|weak: &Weak<Shared>| {
            let weak = weak.clone();
            // On the worker thread: a report after the queue is gone has no one to tell.
            let runner = ThreadRunner::new(Limits::default(), move |id, event| {
                if let Some(shared) = weak.upgrade() {
                    shared.change(|queue| queue.apply(id, event));
                }
            });
            Shared {
                inner: Mutex::new(Inner {
                    queue: Queue::new(),
                    runner,
                }),
                signal: Signal::default(),
            }
        });
        Self { shared }
    }

    /// Queues the request `sheet` makes; refused with the sheet's problems when it has any.
    pub fn add(&self, sheet: &Sheet) -> Result<u64, Failure> {
        let request = sheet.request()?;
        Ok(self.shared.change(|queue| queue.enqueue(request)).value())
    }

    fn id(queue: &Queue, value: u64) -> Result<JobId, Failure> {
        queue.find(value).map(|i| i.id).ok_or_else(not_found)
    }

    pub fn cancel(&self, value: u64) -> Result<(), Failure> {
        self.shared.change(|queue| {
            let id = Self::id(queue, value)?;
            queue.cancel(id);
            Ok(())
        })
    }

    /// Queues a failed or cancelled job again in its row's place; the new id.
    pub fn retry(&self, value: u64) -> Result<u64, Failure> {
        self.shared.change(|queue| {
            let id = Self::id(queue, value)?;
            queue.retry(id).map(JobId::value).ok_or_else(|| {
                Failure::refused("Only a failed or cancelled conversion can be tried again.")
            })
        })
    }

    pub fn remove(&self, value: u64) -> Result<(), Failure> {
        self.shared.change(|queue| {
            let id = Self::id(queue, value)?;
            if queue.remove(id) {
                Ok(())
            } else {
                Err(Failure::refused(
                    "A running conversion cannot be removed. Cancel it first.",
                ))
            }
        })
    }

    pub fn clear_done(&self) {
        self.shared.change(Queue::clear_done);
    }

    pub fn state_json(&self) -> String {
        // The revision first: a change after it makes the next wait return at once.
        let revision = self.shared.signal.revision();
        let guard = self.shared.lock();
        let queue = &guard.queue;
        let s = queue.summary();
        let state = State {
            revision,
            idle: queue.is_idle(),
            summary: Summary {
                pending: s.pending,
                running: s.running,
                finished: s.finished,
                failed: s.failed,
                cancelled: s.cancelled,
            },
            quit_warning: quit_warning(s),
            jobs: queue.items().iter().map(|i| job(queue, i)).collect(),
        };
        drop(guard);
        serde_json::to_string(&state).unwrap_or_default()
    }

    pub fn revision(&self) -> u64 {
        self.shared.signal.revision()
    }

    pub fn wait(&self, since: u64, timeout: Duration) -> u64 {
        self.shared.signal.wait(since, timeout)
    }
}

impl Drop for JobQueue {
    fn drop(&mut self) {
        self.shared.change(Queue::cancel_all);
        // The worker discards its partial output before it reports the end.
        loop {
            let revision = self.shared.signal.revision();
            if self.shared.lock().queue.is_idle() {
                return;
            }
            self.shared.signal.wait(revision, Duration::from_secs(1));
        }
    }
}
