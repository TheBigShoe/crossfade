//! A runner a test (or a screenshot scene) drives by hand: it records what the queue
//! starts and lets the caller say how each job went, with no threads involved.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use crossfade_app::player::{OutputFormat, VirtualOutput, VirtualSink};
use crossfade_app::queue::{JobId, Runner, RunnerEvent};
use crossfade_app::{FsIo, Request};
use crossfade_graph::CancelToken;

use crate::workspace::{AudioOutput, WorkspaceOptions};

/// The scripted audio output: stereo at 48 kHz, pulled by hand.
pub const SCRIPTED_OUTPUT: OutputFormat = OutputFormat {
    sample_rate: 48_000,
    channels: 2,
};

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
    audio: VirtualOutput,
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
    /// Options for a workspace over a scripted runner, the real file system (tests point it
    /// at a temp directory) and a virtual audio output.
    pub fn options() -> (Self, WorkspaceOptions) {
        let starts = Arc::new(Mutex::new(Vec::new()));
        let (events, receiver) = async_channel::unbounded();
        let (sink, audio) = VirtualSink::new(SCRIPTED_OUTPUT);
        let options = WorkspaceOptions {
            runner: Box::new(ScriptedRunner(Arc::clone(&starts))),
            events: receiver,
            io: Arc::new(|| Box::new(FsIo::new())),
            exists: Arc::new(|path| Path::new(path).exists()),
            audio: AudioOutput::Sink(Box::new(sink)),
        };
        (
            Self {
                starts,
                events,
                audio,
            },
            options,
        )
    }

    /// The device end of the workspace's audio output: pull samples as a device would.
    pub fn audio(&self) -> VirtualOutput {
        self.audio.clone()
    }

    /// Every job started so far, oldest first.
    pub fn starts(&self) -> Vec<Start> {
        self.starts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Report how the `start`-th job started is going, as its worker thread would.
    pub fn send(&self, start: usize, event: RunnerEvent) {
        let id = self.starts()[start].id;
        self.events
            .try_send((id, event))
            .expect("the workspace is listening");
    }
}
