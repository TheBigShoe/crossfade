//! Test apparatus, hidden from the documentation: for this crate's tests and its users' (the
//! conformance tier in `crossfade-format`). Nothing here is used by the decoder.
//!
//! - [`bitfile`]: the test vectors' `.bit` (opus_demo framing) and `.dec` (raw PCM) files.
//! - [`compare`]: a port of `opus_compare.c` (2-clause BSD, its notice kept), the vectors'
//!   quality figure Q.
//! - [`ogg`]: Ogg Opus files (RFC 7845) made in code: headers, seeded packets, pages.
//! - [`packet_writer`]: Opus packets from their parts (RFC 6716 section 3, Appendix B).
//! - [`range_encoder`]: the range encoder (RFC 6716 section 5.1), to write what the decoder
//!   reads.
//! - [`vectors`]: a test vector decoded as `opus_demo` does, the final range checked per
//!   packet.

pub mod bitfile;
pub mod compare;
pub mod ogg;
pub mod packet_writer;
pub mod range_encoder;
pub mod vectors;
