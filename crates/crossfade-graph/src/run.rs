use crate::filter::FilterSpec;
use crate::job::Job;
use crate::pipeline::{Pipeline, Plan, Step};
use crate::two_pass::Pass;
use crossfade_core::{Error, JobIo, Limits, MediaSink, MediaSource};
use crossfade_dsp::filters::normalize::to_db;
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
    /// A job that normalises starts by measuring: no output exists until this pass ends.
    /// Its progress is the first half of the job's.
    Measuring,
    /// What the measuring pass found and the gain the conversion will apply.
    Normalized(Normalization),
    /// The conversion starts (after `Normalized` when the job normalises).
    Started {
        plan: Plan,
    },
    Progress {
        frames_done: u64,
        frames_total: Option<u64>,
    },
    /// Samples the output's integer format could not hold, saturated instead; sent once,
    /// before `Finished`, and only when there were any.
    Clipped {
        samples: u64,
    },
    Finished {
        frames: u64,
    },
}

/// The measuring pass's result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Normalization {
    /// Integrated loudness of the signal as it reached the `normalize` stage; `None` for
    /// silence or a programme under 400 ms (the gain is then unity).
    pub measured_lufs: Option<f64>,
    pub target_lufs: f64,
    pub gain_db: f64,
    /// The true-peak ceiling, not the target, set the gain: the target is not reached.
    pub limited_by_ceiling: bool,
    pub ceiling_dbtp: f64,
}

impl Normalization {
    /// The loudness the gain gives, when there was one to measure.
    pub fn reached_lufs(&self) -> Option<f64> {
        self.measured_lufs.map(|l| l + self.gain_db)
    }
}

/// Runs the job to completion. Output becomes visible only through
/// `io.commit` on success; on cancel or any error the partial output is
/// discarded first.
///
/// A job that normalises runs twice over its input: a measuring pass that writes nothing,
/// then the conversion. The two passes share one progress, each half of it (both read and
/// decode the whole input; the conversion's encoding roughly balances the meter's
/// oversampled true-peak work).
pub fn run(
    job: &Job,
    io: &mut dyn JobIo,
    limits: &Limits,
    cancel: &CancelToken,
    on_event: &mut dyn FnMut(&Event),
) -> Result<(), Error> {
    let [output] = job.outputs.as_slice() else {
        return Err(Error::Unsupported {
            what: "exactly one output".into(),
        });
    };
    let normalises = output
        .audio
        .filters
        .iter()
        .any(|f| matches!(f, FilterSpec::Normalize { .. }));
    let (pass, progress) = if normalises {
        on_event(&Event::Measuring);
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let mut measuring = Pipeline::open_pass(job, io, limits, Pass::Measure)?;
        let halves = Progress {
            offset: 0,
            total: measuring.frames_total().map(|t| 2 * t),
        };
        let done = drive(&mut measuring, io, cancel, on_event, halves)?;
        let meter = measuring
            .take_measuring()
            .expect("a measuring pass has a meter");
        let plan = meter.plan;
        let m = meter.finish();
        let gain = plan.gain(&m);
        on_event(&Event::Normalized(Normalization {
            measured_lufs: m.integrated,
            target_lufs: plan.target_lufs,
            gain_db: to_db(gain.gain.linear),
            limited_by_ceiling: gain.limited_by_ceiling,
            ceiling_dbtp: plan.ceiling_dbtp,
        }));
        let second = Progress {
            offset: done,
            total: halves.total,
        };
        (Pass::Convert(Some(gain.gain)), second)
    } else {
        (Pass::Convert(None), Progress::default())
    };
    // `open` discards by itself when it fails after creating the sink.
    let mut pipeline = if normalises {
        Pipeline::open_pass(job, &mut Reopen(io), limits, pass)?
    } else {
        Pipeline::open_pass(job, io, limits, pass)?
    };
    on_event(&Event::Started {
        plan: pipeline.plan().clone(),
    });
    let driven = if normalises {
        drive(&mut pipeline, &mut Reopen(io), cancel, on_event, progress)
    } else {
        drive(&mut pipeline, io, cancel, on_event, progress)
    };
    let result = driven.and_then(|frames| {
        io.commit(&output.path)?;
        Ok(frames)
    });
    let clipped = pipeline.clipped();
    if result.is_ok() && clipped > 0 {
        on_event(&Event::Clipped { samples: clipped });
    }
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

/// How a pass's own progress maps onto the job's.
#[derive(Debug, Clone, Copy, Default)]
struct Progress {
    offset: u64,
    /// The job's total, when it differs from the pass's.
    total: Option<u64>,
}

/// A second pass's I/O: an input that cannot be opened again says why it was asked to be.
struct Reopen<'a>(&'a mut dyn JobIo);

impl JobIo for Reopen<'_> {
    fn open_source(&mut self, path: &str) -> Result<Box<dyn MediaSource>, Error> {
        self.0.open_source(path).map_err(|e| {
            let why = format!("normalising reads {path} twice, and it could not be opened again");
            match e {
                Error::Io { kind, message } => Error::Io {
                    kind,
                    message: format!("{why}: {message}"),
                },
                other => Error::Unsupported {
                    what: format!("{why}: {other}"),
                },
            }
        })
    }

    fn create_sink(&mut self, path: &str) -> Result<Box<dyn MediaSink>, Error> {
        self.0.create_sink(path)
    }

    fn commit(&mut self, path: &str) -> Result<(), Error> {
        self.0.commit(path)
    }

    fn discard(&mut self, path: &str) {
        self.0.discard(path);
    }
}

/// Steps to the end; returns the job's frames done.
fn drive(
    pipeline: &mut Pipeline,
    io: &mut dyn JobIo,
    cancel: &CancelToken,
    on_event: &mut dyn FnMut(&Event),
    progress: Progress,
) -> Result<u64, Error> {
    let mut frames = progress.offset;
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        match pipeline.step_with(io)? {
            Step::Progress {
                frames_done,
                frames_total,
            } => {
                frames = progress.offset + frames_done;
                on_event(&Event::Progress {
                    frames_done: frames,
                    frames_total: progress.total.or(frames_total),
                });
            }
            Step::Finished => return Ok(frames),
        }
    }
}
