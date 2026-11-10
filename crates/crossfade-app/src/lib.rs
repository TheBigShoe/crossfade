//! What every front end shares: file I/O, the job runner, the queue and the
//! screen models. No GUI toolkit here, so the CLI and the app stay in step.
#![forbid(unsafe_code)]

mod fsio;

pub use fsio::FsIo;
