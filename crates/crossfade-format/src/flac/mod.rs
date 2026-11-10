//! FLAC container. The reader and the writer live in separate files.

mod demux;
mod mux;

pub use demux::open;
pub use mux::create;
