use crate::job::Job;
use crate::pipeline::{Pipeline, Plan, Step};
use crossfade_core::{Error, JobIo, Limits};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cooperative cancellation: clones share one flag, `run` polls it between steps.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Started {
        plan: Plan,
    },
    Progress {
        frames_done: u64,
        frames_total: Option<u64>,
    },
    Finished {
        frames: u64,
    },
}

/// Runs the job to completion. Output becomes visible only through
/// `io.commit` on success; on cancel or any error the partial output is
/// discarded first.
pub fn run(
    job: &Job,
    io: &mut dyn JobIo,
    limits: &Limits,
    cancel: &CancelToken,
    on_event: &mut dyn FnMut(&Event),
) -> Result<(), Error> {
    // `open` discards by itself when it fails after creating the sink.
    let mut pipeline = Pipeline::open(job, io, limits)?;
    let [output] = job.outputs.as_slice() else {
        return Err(Error::Unsupported {
            what: "exactly one output".into(),
        });
    };
    let result = drive(&mut pipeline, cancel, on_event).and_then(|frames| {
        io.commit(&output.path)?;
        Ok(frames)
    });
    match result {
        Ok(frames) => {
            on_event(&Event::Finished { frames });
            Ok(())
        }
        Err(e) => {
            io.discard(&output.path);
            Err(e)
        }
    }
}

/// Steps to the end; returns the frames processed.
fn drive(
    pipeline: &mut Pipeline,
    cancel: &CancelToken,
    on_event: &mut dyn FnMut(&Event),
) -> Result<u64, Error> {
    on_event(&Event::Started {
        plan: pipeline.plan().clone(),
    });
    let mut frames = 0;
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        match pipeline.step()? {
            Step::Progress {
                frames_done,
                frames_total,
            } => {
                frames = frames_done;
                on_event(&Event::Progress {
                    frames_done,
                    frames_total,
                });
            }
            Step::Finished => return Ok(frames),
        }
    }
}
