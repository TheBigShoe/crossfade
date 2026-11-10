//! Test support shared across the workspace. Depends only on
//! `crossfade-core` and is only ever a dev-dependency.
#![forbid(unsafe_code)]

pub mod signal;

pub mod clock;
pub mod io;
pub mod memory;
pub mod metrics;
pub mod pcm;
