//! Shared vocabulary of the Crossfade engine: time, audio descriptions and
//! the I/O and clock seams every other engine crate builds on. Pure Rust,
//! no native dependencies.
#![forbid(unsafe_code)]

mod audio;
mod clock;
mod error;
mod io;
mod limits;
mod rational;
mod time;

pub use audio::{AudioSpec, Channel, ChannelLayout, SampleFormat};
pub use clock::{Clock, SystemClock};
pub use error::Error;
pub use io::{MediaSink, MediaSource};
pub use limits::Limits;
pub use rational::Rational;
pub use time::{Rounding, TimeBase, Timestamp};
