//! The processing graph: wires demuxers, decoders, filters, encoders and
//! muxers into a pipeline. The single engine entry point for the CLI and UI.
#![forbid(unsafe_code)]

mod job;
mod pipeline;
mod probe;

pub use crossfade_core::JobIo;
pub use job::{AudioSettings, Job, JobInput, JobOutput};
pub use pipeline::{Pipeline, Plan, Step};
pub use probe::{ProbeReport, probe};
