//! Test-corpus tool: lists the official conformance files in `corpus.toml`,
//! fetches them once into a cache and verifies them by SHA-256. Not an engine
//! crate; nothing in the engine depends on it.
#![forbid(unsafe_code)]

mod cache;
mod manifest;
#[cfg(test)]
mod testutil;

pub use cache::{Cache, Problem};

pub use manifest::{Entry, Expect, Manifest, ManifestError};
