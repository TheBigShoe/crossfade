//! The filters, one module each.

pub mod gain;

use crossfade_core::{AudioFrame, Error};

/// Filters run on `f64`; the chain converts before the first one.
fn not_working_format(filter: &str, frame: &AudioFrame) -> Error {
    Error::Unsupported {
        what: format!(
            "the {filter} filter takes f64 frames, not {}",
            frame.samples.format().name()
        ),
    }
}
