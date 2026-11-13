//! AAC in pure Rust. Always built: the ADTS frame header ([`adts`]) and the
//! AudioSpecificConfig ([`asc`]) parsers, which a container reader needs to split packets.
//! Behind the off-by-default feature `decoder` (AAC is patent-encumbered): an AAC-LC decoder.
//! Depends only on `libm`; no unsafe code.
#![forbid(unsafe_code)]

pub mod adts;
pub mod asc;
mod bits;
#[cfg(feature = "decoder")]
pub mod decoder;
mod error;

#[cfg(feature = "decoder")]
pub use decoder::Decoder;
pub use error::{Error, Result};
