//! The real runner: one worker thread per job over the real file system.

use crate::command::Request;
use crate::fsio::FsIo;
use crate::queue::{JobId, Runner, RunnerEvent};
use crossfade_core::{Error, Limits};
use crossfade_graph::{CancelToken, Event, run};
use std::cell::Cell;
use std::path::Path;
use std::sync::Arc;

/// With an unknown total there is no fraction to show, so report by volume.
const UNKNOWN_TOTAL_STEP: u64 = 65_536;

/// Decides which progress reports are worth sending, from frame counts alone
/// so the cadence is the same on every machine.
struct Throttle {
    last: u64,
}

impl Throttle {
    fn new() -> Self {
        Self { last: 0 }
    }

    fn admit(&mut self, done: u64, total: Option<u64>) -> bool {
        let step = total.map_or(UNKNOWN_TOTAL_STEP, |t| (t / 200).max(1));
        let admit = done >= self.last.saturating_add(step);
        if admit {
            self.last = done;
        }
        admit
    }

    fn last_emitted(&self) -> u64 {
        self.last
    }
}

type Notify = Arc<dyn Fn(JobId, RunnerEvent) + Send + Sync>;

/// Reports a failure if the worker unwinds before saying how the job ended;
/// otherwise a panic would leave the job running forever in the queue.
struct EndGuard<'a> {
    id: JobId,
    ended: Cell<bool>,
    notify: &'a (dyn Fn(JobId, RunnerEvent) + Send + Sync),
}

impl EndGuard<'_> {
    fn send(&self, event: RunnerEvent) {
        if matches!(
            event,
            RunnerEvent::Finished { .. } | RunnerEvent::Failed { .. } | RunnerEvent::Cancelled
        ) {
            self.ended.set(true);
        }
        (self.notify)(self.id, event);
    }
}

impl Drop for EndGuard<'_> {
    fn drop(&mut self) {
        if !self.ended.get() {
            self.send(RunnerEvent::Failed {
                message: "the converter stopped unexpectedly".into(),
            });
        }
    }
}

/// `notify` is called on the worker thread; a UI forwards it to its own. The queue starts one
/// job at a time, so each job may have the whole machine: `limits.max_threads` is passed on
/// as given (the default, 0, lets the encoder choose).
pub struct ThreadRunner {
    limits: Limits,
    notify: Notify,
}

impl ThreadRunner {
    pub fn new(
        limits: Limits,
        notify: impl Fn(JobId, RunnerEvent) + Send + Sync + 'static,
    ) -> Self {
        Self {
            limits,
            notify: Arc::new(notify),
        }
    }
}

impl Runner for ThreadRunner {
    fn start(&mut self, id: JobId, request: &Request, cancel: CancelToken) {
        let (request, limits, notify) = (request.clone(), self.limits, Arc::clone(&self.notify));
        std::thread::spawn(move || work(id, &request, &limits, &cancel, &*notify));
    }
}

fn work(
    id: JobId,
    request: &Request,
    limits: &Limits,
    cancel: &CancelToken,
    notify: &(dyn Fn(JobId, RunnerEvent) + Send + Sync),
) {
    let guard = EndGuard {
        id,
        ended: Cell::new(false),
        notify,
    };
    let send = |event| guard.send(event);
    // Same refusal as the CLI, without its `--force` hint.
    if let ([_], [output]) = (
        request.job.inputs.as_slice(),
        request.job.outputs.as_slice(),
    ) && !request.overwrite
        && Path::new(&output.path).exists()
    {
        send(RunnerEvent::Failed {
            message: format!("{} already exists", output.path),
        });
        return;
    }
    let mut throttle = Throttle::new();
    let mut latest = (0, None);
    let result = run(
        &request.job,
        &mut FsIo::new(),
        limits,
        cancel,
        &mut |event| match event {
            Event::Started { .. } => send(RunnerEvent::Started),
            Event::Progress {
                frames_done,
                frames_total,
            } => {
                latest = (*frames_done, *frames_total);
                if throttle.admit(*frames_done, *frames_total) {
                    send(RunnerEvent::Progress {
                        frames_done: *frames_done,
                        frames_total: *frames_total,
                    });
                }
            }
            Event::Measuring => send(RunnerEvent::Measuring),
            Event::Normalized(found) => send(RunnerEvent::Normalized(*found)),
            Event::Clipped { samples } => send(RunnerEvent::Clipped { samples: *samples }),
            Event::Finished { frames } => {
                // The last state always goes out, whatever the throttle said.
                if latest.0 > throttle.last_emitted() {
                    send(RunnerEvent::Progress {
                        frames_done: latest.0,
                        frames_total: latest.1,
                    });
                }
                send(RunnerEvent::Finished { frames: *frames });
            }
        },
    );
    match result {
        Ok(()) => {}
        Err(Error::Cancelled) => send(RunnerEvent::Cancelled),
        Err(e) => send(RunnerEvent::Failed {
            message: e.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::Throttle;

    #[test]
    fn known_total_emits_every_two_hundredth() {
        let mut t = Throttle::new();
        assert!(!t.admit(4, Some(1000)));
        assert!(t.admit(5, Some(1000)));
        assert!(!t.admit(9, Some(1000)));
        assert!(t.admit(10, Some(1000)));
    }

    #[test]
    fn a_small_total_emits_every_advance() {
        let mut t = Throttle::new();
        assert!(t.admit(1, Some(10)));
        assert!(!t.admit(1, Some(10)));
        assert!(t.admit(2, Some(10)));
    }

    #[test]
    fn unknown_total_emits_every_65536_frames() {
        let mut t = Throttle::new();
        assert!(!t.admit(65_535, None));
        assert!(t.admit(65_536, None));
        assert!(!t.admit(65_536 + 65_535, None));
        assert!(t.admit(131_072, None));
    }

    #[test]
    fn the_last_emitted_position_is_remembered() {
        let mut t = Throttle::new();
        assert_eq!(t.last_emitted(), 0);
        t.admit(7, Some(10));
        assert_eq!(t.last_emitted(), 7);
        t.admit(7, Some(1_000_000));
        assert_eq!(t.last_emitted(), 7);
    }
}
