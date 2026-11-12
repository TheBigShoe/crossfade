//! Test-corpus tool: lists the official conformance files in `corpus.toml`,
//! fetches them once into a cache and verifies them by SHA-256. Not an engine
//! crate; nothing in the engine depends on it.
#![forbid(unsafe_code)]

mod archive;
mod cache;
mod cli;
mod fetch;
mod manifest;
mod require;
#[cfg(test)]
mod testutil;

pub use cache::{Cache, Problem};
pub use cli::{Defaults, run};
pub use fetch::{
    CurlDownloader, Downloader, Event, Failure, Outcome, Received, Summary, fetch, seed, selected,
};

pub use manifest::{Archive, ArchiveFormat, Entry, Expect, Manifest, ManifestError, Source};
pub use require::{RequireError, require, require_from, try_require, workspace_root};
