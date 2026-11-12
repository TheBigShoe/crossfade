//! The processing graph: wires demuxers, decoders, filters, encoders and
//! muxers into a pipeline. The single engine entry point for the CLI and UI.
#![forbid(unsafe_code)]

mod choice;
mod decode;
pub mod filter;
mod job;
mod measure;
mod pipeline;
mod probe;
mod run;
mod timeline;
mod two_pass;
mod validate;

pub use choice::{can_hold, can_write, encode_options, output_codec, writable_extensions};
pub use crossfade_core::JobIo;
pub use crossfade_dsp::dither::Dither;
pub use decode::decode_frames;
pub use job::{AudioSettings, Job, JobInput, JobOutput};
pub use measure::{Measurements, measure_loudness};
pub use pipeline::{Pipeline, Plan, Step};
pub use probe::{
    Loudness, ProbeDocument, ProbeOptions, ProbeReport, ProbeStream, ProbeTag, probe, probe_with,
    probe_with_cancel,
};
pub use run::{CancelToken, Event, Normalization, run};
pub use timeline::Timeline;
pub use validate::{OutputPlan, describe, describe_concat, processing, validate, validate_concat};
