//! The job queue: pure state, driven by a front end. The queue never starts
//! work on its own; the caller pumps it and feeds back what the runner says,
//! which keeps it testable without threads.

use crate::command::{Request, command_line};
use crossfade_graph::CancelToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(u64);

#[derive(Debug, Clone, PartialEq)]
pub enum JobState {
    Pending,
    Running {
        frames_done: u64,
        frames_total: Option<u64>,
        cancelling: bool,
    },
    Finished {
        frames: u64,
    },
    Failed {
        message: String,
    },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunnerEvent {
    Started,
    Progress {
        frames_done: u64,
        frames_total: Option<u64>,
    },
    /// Samples the output's format saturated; sent before `Finished`, only when there were any.
    Clipped {
        samples: u64,
    },
    Finished {
        frames: u64,
    },
    Failed {
        message: String,
    },
    Cancelled,
}

pub trait Runner {
    fn start(&mut self, id: JobId, request: &Request, cancel: CancelToken);
}

#[derive(Debug, Clone)]
pub struct QueueItem {
    pub id: JobId,
    pub request: Request,
    pub state: JobState,
    /// Samples the finished job saturated; 0 when it did not clip.
    pub clipped: u64,
    token: Option<CancelToken>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    pub pending: usize,
    pub running: usize,
    pub finished: usize,
    pub failed: usize,
    pub cancelled: usize,
}

/// What a front end tells the user before quitting would discard work: `None` when no
/// job is running or waiting, so there is nothing to ask about.
pub fn quit_warning(summary: Summary) -> Option<String> {
    let total = summary.running + summary.pending;
    let state = match (summary.running, summary.pending) {
        (0, 0) => return None,
        (_, 0) => "running",
        (0, _) => "waiting",
        _ => "running or waiting",
    };
    Some(if total == 1 {
        format!("1 conversion is still {state}. Quitting will cancel it.")
    } else {
        format!("{total} conversions are still {state}. Quitting will cancel them.")
    })
}

#[derive(Debug, Default)]
pub struct Queue {
    items: Vec<QueueItem>,
    next_id: u64,
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(&mut self, request: Request) -> JobId {
        let id = JobId(self.next_id);
        self.next_id += 1;
        self.items.push(QueueItem {
            id,
            request,
            state: JobState::Pending,
            clipped: 0,
            token: None,
        });
        id
    }

    /// Starts the oldest pending job unless one is already running.
    pub fn pump(&mut self, runner: &mut dyn Runner) {
        if self
            .items
            .iter()
            .any(|i| matches!(i.state, JobState::Running { .. }))
        {
            return;
        }
        let Some(item) = self.items.iter_mut().find(|i| i.state == JobState::Pending) else {
            return;
        };
        let token = CancelToken::new();
        item.token = Some(token.clone());
        item.state = JobState::Running {
            frames_done: 0,
            frames_total: None,
            cancelling: false,
        };
        runner.start(item.id, &item.request, token);
    }

    /// Events only mean something for a running job; anything else is a late
    /// or stray report and is dropped.
    pub fn apply(&mut self, id: JobId, event: RunnerEvent) {
        let Some(item) = self.item_mut(id) else {
            return;
        };
        let JobState::Running { cancelling, .. } = item.state else {
            return;
        };
        let next = match event {
            RunnerEvent::Started => return,
            RunnerEvent::Clipped { samples } => {
                item.clipped = samples;
                return;
            }
            RunnerEvent::Progress {
                frames_done,
                frames_total,
            } => JobState::Running {
                frames_done,
                frames_total,
                cancelling,
            },
            RunnerEvent::Finished { frames } => JobState::Finished { frames },
            RunnerEvent::Failed { message } => JobState::Failed { message },
            RunnerEvent::Cancelled => JobState::Cancelled,
        };
        if !matches!(next, JobState::Running { .. }) {
            item.token = None;
        }
        item.state = next;
    }

    pub fn cancel(&mut self, id: JobId) {
        let Some(item) = self.item_mut(id) else {
            return;
        };
        match &mut item.state {
            JobState::Pending => item.state = JobState::Cancelled,
            JobState::Running { cancelling, .. } => {
                *cancelling = true;
                if let Some(token) = &item.token {
                    token.cancel();
                }
            }
            _ => {}
        }
    }

    /// For quitting: stop the running job and drop the waiting ones.
    pub fn cancel_all(&mut self) {
        let ids: Vec<JobId> = self.items.iter().map(|i| i.id).collect();
        for id in ids {
            self.cancel(id);
        }
    }

    /// Nothing waiting or running, so nothing can be left half-written. A cancelled
    /// running job counts as running until its worker reports the end.
    pub fn is_idle(&self) -> bool {
        !self
            .items
            .iter()
            .any(|i| matches!(i.state, JobState::Pending | JobState::Running { .. }))
    }

    pub fn retry(&mut self, id: JobId) -> Option<JobId> {
        let item = self.get(id)?;
        matches!(item.state, JobState::Failed { .. } | JobState::Cancelled)
            .then(|| item.request.clone())
            .map(|request| self.enqueue(request))
    }

    pub fn remove(&mut self, id: JobId) -> bool {
        let Some(at) = self.items.iter().position(|i| i.id == id) else {
            return false;
        };
        if matches!(self.items[at].state, JobState::Running { .. }) {
            return false;
        }
        self.items.remove(at);
        true
    }

    pub fn clear_done(&mut self) {
        self.items
            .retain(|i| !matches!(i.state, JobState::Finished { .. }));
    }

    pub fn items(&self) -> &[QueueItem] {
        &self.items
    }

    pub fn get(&self, id: JobId) -> Option<&QueueItem> {
        self.items.iter().find(|i| i.id == id)
    }

    fn item_mut(&mut self, id: JobId) -> Option<&mut QueueItem> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    /// 0 to 1; `None` when the total is unknown or the job ended unfinished.
    pub fn fraction(&self, id: JobId) -> Option<f32> {
        match self.get(id)?.state {
            JobState::Pending => Some(0.0),
            JobState::Finished { .. } => Some(1.0),
            JobState::Running {
                frames_done,
                frames_total: Some(total),
                ..
            } => Some(if total == 0 {
                1.0
            } else {
                (frames_done as f64 / total as f64).clamp(0.0, 1.0) as f32
            }),
            _ => None,
        }
    }

    pub fn command_line(&self, id: JobId) -> Option<String> {
        command_line(&self.get(id)?.request).ok()
    }

    pub fn summary(&self) -> Summary {
        let mut s = Summary::default();
        for item in &self.items {
            match item.state {
                JobState::Pending => s.pending += 1,
                JobState::Running { .. } => s.running += 1,
                JobState::Finished { .. } => s.finished += 1,
                JobState::Failed { .. } => s.failed += 1,
                JobState::Cancelled => s.cancelled += 1,
            }
        }
        s
    }
}
