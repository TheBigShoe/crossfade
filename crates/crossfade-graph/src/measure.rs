//! Loudness and peaks of a file: one decode pass through the meter. What the pass holds is the
//! meter's history, 16 bytes per 100 ms of programme (about 0.6 MB an hour), never samples.

use crate::decode::decode_frames;
use crate::run::CancelToken;
use crossfade_core::{Error, JobIo, Limits};
use crossfade_dsp::filters::normalize::{Measure, Options};

pub use crossfade_dsp::filters::normalize::Measurements;

/// Measures the one audio stream of `path` as it stands (no filters). Cancel is polled
/// between packets.
pub fn measure_loudness(
    path: &str,
    io: &mut dyn JobIo,
    limits: &Limits,
    cancel: &CancelToken,
) -> Result<Measurements, Error> {
    let mut measure: Option<Measure> = None;
    let stream = decode_frames(path, io, limits, cancel, &mut |stream, frame| {
        if measure.is_none() {
            measure = Some(Measure::new(&stream.audio, Options::default())?);
        }
        measure.as_mut().expect("just set").push(frame)
    })?;
    // A stream with no frames still has to be one the meter can take.
    match measure {
        Some(m) => Ok(m.finish()),
        None => Ok(Measure::new(&stream.audio, Options::default())?.finish()),
    }
}
