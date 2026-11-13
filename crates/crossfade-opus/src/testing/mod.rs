//! Test apparatus, hidden from the documentation: for this crate's tests and its users' (the
//! conformance tier in `crossfade-format`). Nothing here is used by the decoder.
//!
//! - [`range_encoder`]: the range encoder (RFC 6716 section 5.1), to write what the decoder
//!   reads.

pub mod range_encoder;
