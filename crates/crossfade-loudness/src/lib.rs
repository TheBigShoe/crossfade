//! Programme loudness and peak measurement to ITU-R BS.1770-4 and EBU R 128: integrated,
//! momentary and short-term loudness (EBU Tech 3341), loudness range (Tech 3342), sample peak
//! and true peak, at any rate from 8,000 to 1,048,575 Hz.
//!
//! ```
//! use crossfade_loudness::{ChannelRole, Meter, Options, normalisation_gain, to_db};
//!
//! let mut meter = Meter::new(48_000, &[ChannelRole::Left, ChannelRole::Right], Options::default())?;
//! // Interleaved samples, any chunk size (even half a frame); here 5 s of a 1 kHz tone.
//! let tone: Vec<f64> = (0..48_000 * 5)
//!     .flat_map(|i| {
//!         let s = 0.07 * (2.0 * std::f64::consts::PI * 1000.0 * f64::from(i) / 48_000.0).sin();
//!         [s, s]
//!     })
//!     .collect();
//! meter.push(&tone);
//! let m = meter.finish();
//! let lufs = m.integrated.expect("5 s of tone is loud enough to measure");
//! assert!((lufs + 23.0).abs() < 0.1);
//! let gain = normalisation_gain(m.integrated, -16.0, m.true_peak.unwrap(), -1.0);
//! assert!(gain.linear > 1.0 && to_db(gain.linear * m.true_peak.unwrap()) <= -0.999);
//! # Ok::<(), crossfade_loudness::Error>(())
//! ```
//!
//! # Same bits everywhere
//!
//! A loudness figure feeds a normalising gain, which changes output bytes, so a result must not
//! depend on the platform. `std`'s `sin`, `cos`, `tan`, `exp`, `ln`, `log10` and `powf` are
//! not guaranteed to agree between platforms, so this crate uses none of them, and no
//! `mul_add`: only `+ - * /`, `sqrt`, comparisons, conversions and the pure-Rust `libm` crate
//! (its only dependency). The result is also independent of how the input is chunked.
//!
//! # What is reported when there is nothing to report
//!
//! Loudness values are `Option<f64>` in LUFS (LU for the range): `None` means "no loudness":
//! fewer than 400 ms pushed (3 s for short-term and range), digital silence, or everything
//! under the -70 LUFS absolute gate. Peaks are linear and `0.0` for silence; [`to_db`] maps that
//! to negative infinity. Nothing is ever NaN.
//!
//! # Input that is not a number
//!
//! NaN and infinities count as 0.0, and finite samples beyond +-1e12 are clamped to it (so
//! a block sum cannot overflow); [`Meter::sanitised_samples`] counts the replacements, so a
//! caller can tell. Later results are measured on the replaced signal and are otherwise normal.
//!
//! # Memory
//!
//! Samples are never stored. The only state that grows is one 8-byte power per 100 ms for the
//! gating blocks above -70 LUFS and one more for the range: at most 16 bytes per 100 ms, about
//! 14 MB for 24 hours (a `Vec` may hold twice that). Silence stores nothing; switching the range
//! off halves the growth ([`Meter::heap_bytes`]).
//!
//! # Mono
//!
//! A mono signal is one channel counted once. EBU Tech 3341 and 3342 contain no "dual mono"
//! (+3 LU) convention, so none is offered: to measure it as two loudspeakers, push the channel
//! twice.
#![forbid(unsafe_code)]

mod channel;
mod error;
mod gain;
mod kweight;
mod meter;
mod peak;

/// Most channels a meter accepts.
pub const MAX_CHANNELS: usize = 64;

pub use channel::ChannelRole;
pub use error::{Error, Result};
pub use gain::{Gain, normalisation_gain};
pub use kweight::{Biquad, KWeighting, MAX_SAMPLE_RATE, MIN_SAMPLE_RATE};
pub use meter::{Measurements, Meter, Options, to_db};
