//! The C interface the native mobile apps call: opaque handles over the `crossfade-app`
//! models, state and changes as JSON, and the player's wait-free pull for the platform's
//! audio callback. `include/crossfade.h` declares it; `README.md` is the contract.
//!
//! Only the `abi` module may hold `unsafe`, and only to read what a caller's pointer points
//! at or to take and give back a box; the logic is in safe modules it calls.
#![deny(unsafe_code)]

mod about;
mod abi;
mod error;
mod inspect;

pub use abi::*;
pub use error::*;
