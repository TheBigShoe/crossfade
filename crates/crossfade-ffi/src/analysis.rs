//! Background analysis across the boundary: the app's `Analyzer` for one file, its snapshot
//! and readout as JSON, and the waveform's columns into the caller's buffers.

use crate::error::Failure;
use crate::signal::Signal;
use crossfade_app::FsIo;
use crossfade_app::analysis::{Analyzer, State};
use crossfade_app::waveview::{LanePlan, readout};
use crossfade_core::Limits;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

/// One file's analysis, running on its own thread from the start. Dropping it cancels it.
pub struct FileAnalysis {
    analyzer: Analyzer,
    signal: Arc<Signal>,
}

#[derive(Serialize)]
struct Loudness {
    integrated: Option<f64>,
    range: Option<f64>,
    true_peak: Option<f64>,
    sample_peak: f64,
}

#[derive(Serialize)]
struct Segment {
    text: String,
    warn: bool,
}

#[derive(Serialize)]
struct Readout {
    text: String,
    warn: bool,
    segments: Vec<Segment>,
}

#[derive(Serialize)]
struct StateJson {
    revision: u64,
    /// `running`, `done`, `failed` or `cancelled`.
    state: &'static str,
    failure: Option<String>,
    frames_done: u64,
    frames: Option<u64>,
    sample_rate: Option<u32>,
    channels: Option<u16>,
    lanes: usize,
    loudness: Loudness,
    readout: Readout,
}

impl FileAnalysis {
    pub fn start(path: &str) -> Self {
        let signal = Arc::new(Signal::default());
        let notify = Arc::clone(&signal);
        let analyzer = Analyzer::start(path, Box::new(FsIo::new()), Limits::default(), move |_| {
            notify.bump();
        });
        Self { analyzer, signal }
    }

    pub fn cancel(&self) {
        self.analyzer.cancel();
    }

    pub fn wait(&self, since: u64, timeout: Duration) -> u64 {
        self.signal.wait(since, timeout)
    }

    pub fn state_json(&self) -> String {
        let revision = self.signal.revision();
        let snapshot = self.analyzer.snapshot();
        let lanes = self
            .analyzer
            .with(|a| LanePlan::for_channels(a.pyramid.channels()).lanes());
        let line = readout(&snapshot);
        let (state, failure) = match &snapshot.state {
            State::Running => ("running", None),
            State::Done => ("done", None),
            State::Failed(why) => ("failed", Some(why.clone())),
            State::Cancelled => ("cancelled", None),
        };
        let stream = snapshot.stream.as_ref();
        let json = StateJson {
            revision,
            state,
            failure,
            frames_done: snapshot.frames_done,
            // As the strip: the length the stream states, else what has been read so far.
            frames: stream.map(|s| s.frames.unwrap_or(snapshot.frames_done)),
            sample_rate: stream.map(|s| s.audio.sample_rate()),
            channels: stream.map(|s| s.audio.layout().channels()),
            lanes,
            loudness: Loudness {
                integrated: snapshot.loudness.integrated,
                range: snapshot.loudness.range,
                true_peak: snapshot.loudness.true_peak,
                sample_peak: snapshot.loudness.sample_peak,
            },
            readout: Readout {
                text: line.text(),
                warn: line.warns(),
                segments: line
                    .segments
                    .into_iter()
                    .map(|s| Segment {
                        text: s.text,
                        warn: s.warn,
                    })
                    .collect(),
            },
        };
        serde_json::to_string(&json).unwrap_or_default()
    }

    /// Writes `mins.len()` columns of `frames_per_column` frames from `start` for `lane` of the
    /// strip's lane plan, NaN past what is analysed; the number with data, a prefix.
    pub fn columns(
        &self,
        lane: usize,
        start: u64,
        frames_per_column: u64,
        mins: &mut [f32],
        maxs: &mut [f32],
    ) -> Result<usize, Failure> {
        let n = mins.len().min(maxs.len());
        let columns = self.analyzer.with(|a| {
            let p = &a.pyramid;
            match LanePlan::for_channels(p.channels()) {
                LanePlan::Combined if lane == 0 => {
                    Some(p.columns_union(start, frames_per_column, n))
                }
                LanePlan::Separate(lanes) if lane < lanes => {
                    Some(p.columns(lane, start, frames_per_column, n))
                }
                _ => None,
            }
        });
        let Some(columns) = columns else {
            return Err(Failure::refused(format!(
                "The waveform has no lane {lane}."
            )));
        };
        let mut with_data = 0;
        for (i, column) in columns.into_iter().enumerate() {
            let (lo, hi) = column.unwrap_or((f32::NAN, f32::NAN));
            mins[i] = lo;
            maxs[i] = hi;
            if column.is_some() {
                with_data = i + 1;
            }
        }
        Ok(with_data)
    }
}
