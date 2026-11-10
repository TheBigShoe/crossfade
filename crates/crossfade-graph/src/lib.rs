//! The processing graph: wires demuxers, decoders, filters, encoders and
//! muxers into a pipeline. The single engine entry point for the CLI and UI.
#![forbid(unsafe_code)]

mod choice;
mod job;
mod pipeline;
mod probe;
mod run;
mod validate;

pub use choice::{can_hold, can_write, encode_options, output_codec, writable_extensions};
pub use crossfade_core::JobIo;
pub use job::{AudioSettings, Job, JobInput, JobOutput};
pub use pipeline::{Pipeline, Plan, Step};
pub use probe::{ProbeDocument, ProbeReport, ProbeStream, ProbeTag, probe};
pub use run::{CancelToken, Event, run};
pub use validate::{OutputPlan, validate};
