//! What every front end shares: file I/O, the job runner, the queue and the
//! screen models. No GUI toolkit here, so the CLI and the app stay in step.
#![forbid(unsafe_code)]

pub mod command;
pub mod fsio;
pub mod inputs;
pub mod inspector;
pub mod sheet;

pub use command::{NotExpressible, Request, command_line, convert_args};
pub use fsio::FsIo;
