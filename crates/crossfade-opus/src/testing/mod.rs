//! Test apparatus, hidden from the documentation: for this crate's tests and its users' (the
//! conformance tier in `crossfade-format`). Nothing here is used by the decoder.
//!
//! - [`range_encoder`]: the range encoder (RFC 6716 section 5.1), to write what the decoder
//!   reads.
//! - [`packet_writer`]: Opus packets from their parts (RFC 6716 section 3, Appendix B).

pub mod packet_writer;
pub mod range_encoder;
