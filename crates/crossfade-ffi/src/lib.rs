//! The C interface the native mobile apps call: opaque handles over the `crossfade-app`
//! models, state and changes as JSON, and the player's wait-free pull for the platform's
//! audio callback. `include/crossfade.h` declares it; `README.md` is the contract.
//!
//! Only the `abi` module may hold `unsafe`, and only to read what a caller's pointer points
//! at or to take and give back a box; the logic is in safe modules it calls.
#![deny(unsafe_code)]

mod abi;
mod about;
mod analysis;
mod error;
mod inspect;
mod player;
mod queue;
mod sheet;
mod signal;

pub use abi::*;
pub use error::*;

/// Every exported function, the one list the header and the sources are checked against.
pub const EXPORTS: &[&str] = &[
    "cf_abi_version",
    "cf_version",
    "cf_about",
    "cf_string_free",
    "cf_last_error_message",
    "cf_inspect",
    "cf_sheet_new",
    "cf_sheet_apply",
    "cf_sheet_state",
    "cf_sheet_destroy",
    "cf_queue_new",
    "cf_queue_add",
    "cf_queue_state",
    "cf_queue_wait",
    "cf_queue_cancel",
    "cf_queue_retry",
    "cf_queue_remove",
    "cf_queue_clear_done",
    "cf_queue_destroy",
    "cf_analysis_new",
    "cf_analysis_state",
    "cf_analysis_wait",
    "cf_analysis_columns",
    "cf_analysis_cancel",
    "cf_analysis_destroy",
    "cf_player_new",
    "cf_player_take_feed",
    "cf_player_load",
    "cf_player_play",
    "cf_player_pause",
    "cf_player_toggle",
    "cf_player_stop",
    "cf_player_seek",
    "cf_player_scrub",
    "cf_player_end_scrub",
    "cf_player_settle",
    "cf_player_position",
    "cf_player_state",
    "cf_player_wait",
    "cf_player_destroy",
    "cf_feed_fill",
    "cf_feed_fill_planar",
    "cf_feed_channels",
    "cf_feed_destroy",
];
