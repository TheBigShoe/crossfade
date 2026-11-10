//! What every front end shares: file I/O, the job runner, the queue and the
//! screen models. No GUI toolkit here, so the CLI and the app stay in step.
#![forbid(unsafe_code)]

pub mod command;
pub mod fsio;
pub mod inspector;

pub use command::{NotExpressible, Request, command_line, convert_args};
pub use fsio::FsIo;
