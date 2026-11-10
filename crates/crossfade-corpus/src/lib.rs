//! Test-corpus tool: lists the official conformance files in `corpus.toml`,
//! fetches them once into a cache and verifies them by SHA-256. Not an engine
//! crate; nothing in the engine depends on it.
#![forbid(unsafe_code)]

mod cache;
mod cli;
mod fetch;
mod manifest;
mod require;
#[cfg(test)]
mod testutil;

pub use cache::{Cache, Problem};
pub use cli::{run, Defaults};
pub use fetch::{fetch, seed, CurlDownloader, Downloader, Event, Failure, Outcome, Summary};

pub use require::{require, require_from, try_require, workspace_root, RequireError};
pub use manifest::{Entry, Expect, Manifest, ManifestError};
