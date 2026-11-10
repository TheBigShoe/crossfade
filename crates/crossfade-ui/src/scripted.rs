//! A runner a test (or a screenshot scene) drives by hand: it records what the queue
//! starts and lets the caller say how each job went, with no threads involved.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use crossfade_app::queue::{JobId, Runner, RunnerEvent};
use crossfade_app::{FsIo, Request};
use crossfade_graph::CancelToken;

use crate::workspace::WorkspaceOptions;

/// One job the queue handed to the runner.
#[derive(Debug, Clone)]
pub struct Start {
    pub id: JobId,
    pub request: Request,
    pub cancel: CancelToken,
}

/// The caller's end of a scripted runner.
#[derive(Clone)]
pub struct Script {
    starts: Arc<Mutex<Vec<Start>>>,
    events: async_channel::Sender<(JobId, RunnerEvent)>,
}

struct ScriptedRunner(Arc<Mutex<Vec<Start>>>);

impl Runner for ScriptedRunner {
    fn start(&mut self, id: JobId, request: &Request, cancel: CancelToken) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Start {
                id,
                request: request.clone(),
                cancel,
            });
    }
}

impl Script {
    /// Options for a workspace over a scripted runner and the real file system (tests
    /// point it at a temp directory).
    pub fn options() -> (Self, WorkspaceOptions) {
        let starts = Arc::new(Mutex::new(Vec::new()));
        let (events, receiver) = async_channel::unbounded();
        let options = WorkspaceOptions {
            runner: Box::new(ScriptedRunner(Arc::clone(&starts))),
            events: receiver,
            io: Arc::new(|| Box::new(FsIo::new())),
            exists: Arc::new(|path| Path::new(path).exists()),
        };
        (Self { starts, events }, options)
    }

    /// Every job started so far, oldest first.
    pub fn starts(&self) -> Vec<Start> {
        self.starts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Report how job `id` is going, as the worker thread would.
    pub fn send(&self, id: JobId, event: RunnerEvent) {
        self.events
            .try_send((id, event))
            .expect("the workspace is listening");
    }
}
