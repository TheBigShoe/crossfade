//! Sample-rate conversion in pure Rust: band-limited (windowed-sinc, linear-phase)
//! polyphase interpolation between any two rates from 1 Hz to [`MAX_RATE`], streaming, for
//! up to [`MAX_CHANNELS`] channels of `f64`. Depends only on `libm`; no unsafe code.
//!
//! ```
//! use crossfade_resample::{Quality, Resampler, output_len};
//!
//! let mut r = Resampler::new(44_100, 48_000, 2, Quality::Balanced)?;
//! let input = vec![0.25; 2 * 44_100]; // one second of interleaved stereo
//! let mut output = Vec::new();
//! for chunk in input.chunks(2 * 1000) {
//!     r.process(chunk, &mut output)?; // any whole-frame chunk, empty included
//! }
//! r.flush(&mut output); // the tail; the resampler is then ready for a new stream
//! assert_eq!(output.len() as u64, 2 * output_len(44_100, 44_100, 48_000));
//! assert_eq!(output.len(), 2 * 48_000);
//! # Ok::<(), crossfade_resample::Error>(())
//! ```
//!
//! # Contract
//!
//! - **Time-aligned**: the filter's delay is compensated, so output frame `n` is the input
//!   signal at time `n / output_rate`; output 0 is input 0. Before the stream and after it
//!   the input counts as silence.
//! - **Length**: `N` input frames become exactly [`output_len`]`(N) = ceil(N * out / in)`
//!   frames: every output instant inside `[0, N / in_rate)`.
//! - **Latency**: [`Resampler::latency`] input frames of lookahead; `process` appends only
//!   outputs whose whole kernel has arrived, `flush` the rest.
//! - **Chunking never changes the output**: the bits are the same however the input is split
//!   into `process` calls, interleaved or planar.
//! - **Same bits on every platform**: the filter is designed with `+ - * /`, `sqrt` and
//!   `libm` only (never `std`'s `sin`, `exp` and friends, whose results may differ between
//!   platforms), and no fused multiply-add is used anywhere. Every sum has an order fixed by
//!   the source: the dot product keeps 16 independent lanes and combines them in a written-out
//!   tree, so vectorisation cannot reorder it. A test pins the hash of fixed conversions.
//! - **Equal rates** pass samples through bit for bit, with no latency.
//! - **Bounded**: tables, history and accumulators are allocated at construction (see
//!   [`Resampler::memory_bytes`]); `process` allocates nothing but growth of the caller's
//!   output vector. Construction evaluates the kernel at most 2^18 times (under 45 ms for any
//!   pair on an Apple M1 Max, 2.6 ms for 44.1 kHz -> 48 kHz at the default).
//! - **Non-finite input** (NaN, infinities) is not an error: it spoils the outputs whose
//!   kernel reaches it (about `latency()` input frames either side), then the stream
//!   recovers exactly. Nothing panics. Finite input within +-1e300 gives finite output.
//!
//! # Design
//!
//! The ratio is reduced by its GCD to `up / down`. Up to [`MAX_GATHER_DECIMATION`]:1
//! decimation, each output is a dot product of one table row with the input around it (the
//! direct form). Beyond that, each input is spread over the outputs it reaches (the transposed
//! form), so memory follows the kernel's width in output samples, not input samples. The
//! table holds every phase exactly while phases x taps fits in 2^18 coefficients; past that
//! (44.1 kHz -> 44.101 kHz, 1 Hz -> 1,048,575 Hz) it holds a fixed number of rows per sample
//! and interpolates between them with a 4-point Lagrange cubic. The kernel is a sinc under a
//! Kaiser window with its pedestal removed (so it reaches zero continuously), cut off midway
//! through the transition band, which ends at the lower rate's Nyquist frequency. Rows are
//! normalised so DC passes with a gain of 1 to within 1e-12. The crate's README has the
//! measured specification of each [`Quality`].
#![forbid(unsafe_code)]

mod design;
mod error;
mod gather;
mod io;
mod kernel;
mod quality;
mod sample;
mod scatter;
mod table;

pub use error::{Error, Result};
pub use quality::Quality;
pub use sample::Sample;

use io::{Input, Output};

/// The highest sample rate accepted, in Hz (FLAC's 20-bit limit).
pub const MAX_RATE: u32 = 1_048_575;
/// The most channels one resampler takes.
pub const MAX_CHANNELS: usize = 256;
/// The largest input-to-output ratio converted in the direct (gather) form.
pub const MAX_GATHER_DECIMATION: u64 = 64;

/// `ceil(frames * up / down)`, saturating.
pub(crate) fn ceil_ratio(frames: u64, up: u64, down: u64) -> u64 {
    let n = (u128::from(frames) * u128::from(up)).div_ceil(u128::from(down));
    u64::try_from(n).unwrap_or(u64::MAX)
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The number of output frames `frames` input frames become: `ceil(frames * out / in)`,
/// saturating at `u64::MAX`; 0 when `input_rate` is 0.
pub fn output_len(frames: u64, input_rate: u32, output_rate: u32) -> u64 {
    if input_rate == 0 {
        return 0;
    }
    ceil_ratio(frames, u64::from(output_rate), u64::from(input_rate))
}

/// A streaming sample-rate converter for one stream of `channels` channels.
///
/// Generic over the sample type so that an `f32` path can be added; today only `f64`
/// implements [`Sample`].
#[derive(Debug, Clone)]
pub struct Resampler<S: Sample = f64> {
    input_rate: u32,
    output_rate: u32,
    channels: usize,
    quality: Quality,
    engine: Engine<S>,
}

#[derive(Debug, Clone)]
enum Engine<S> {
    Pass,
    Gather(gather::Gather<S>),
    Scatter(scatter::Scatter<S>),
}

impl Resampler {
    /// A converter from `input_rate` to `output_rate` Hz for interleaved or planar `f64`.
    pub fn new(
        input_rate: u32,
        output_rate: u32,
        channels: usize,
        quality: Quality,
    ) -> Result<Self> {
        Self::build(input_rate, output_rate, channels, quality)
    }
}

impl<S: Sample> Resampler<S> {
    fn build(input_rate: u32, output_rate: u32, channels: usize, quality: Quality) -> Result<Self> {
        for rate in [input_rate, output_rate] {
            if rate == 0 || rate > MAX_RATE {
                return Err(Error::InvalidRate(rate));
            }
        }
        if channels == 0 || channels > MAX_CHANNELS {
            return Err(Error::InvalidChannels(channels));
        }
        let engine = if input_rate == output_rate {
            Engine::Pass
        } else {
            let g = gcd(u64::from(input_rate), u64::from(output_rate));
            let (up, down) = (u64::from(output_rate) / g, u64::from(input_rate) / g);
            if down > up.saturating_mul(MAX_GATHER_DECIMATION) {
                Engine::Scatter(scatter::Scatter::new(&quality.params(), up, down, channels))
            } else {
                Engine::Gather(gather::Gather::new(&quality.params(), up, down, channels))
            }
        };
        Ok(Self {
            input_rate,
            output_rate,
            channels,
            quality,
            engine,
        })
    }

    pub fn input_rate(&self) -> u32 {
        self.input_rate
    }

    pub fn output_rate(&self) -> u32 {
        self.output_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    /// Input frames of lookahead: nothing comes out until `latency() + 1` frames have gone in.
    pub fn latency(&self) -> u64 {
        match &self.engine {
            Engine::Pass => 0,
            Engine::Gather(g) => g.latency(),
            Engine::Scatter(s) => s.latency(),
        }
    }

    /// Bytes held for tables, history and accumulators: fixed at construction. For any rates
    /// and quality, at most about 2.4 MiB plus 190 KiB per channel (Best at 64:1 decimation;
    /// 44.1 kHz to 48 kHz at the default holds 0.25 MiB plus 10 KiB per channel).
    pub fn memory_bytes(&self) -> usize {
        let samples = match &self.engine {
            Engine::Pass => 0,
            Engine::Gather(g) => g.memory(),
            Engine::Scatter(s) => s.memory(),
        };
        samples * size_of::<S>()
    }

    /// Converts interleaved frames and appends the output frames now complete to `output`.
    pub fn process(&mut self, input: &[S], output: &mut Vec<S>) -> Result<()> {
        if !input.len().is_multiple_of(self.channels) {
            return Err(Error::PartialFrame {
                samples: input.len(),
                channels: self.channels,
            });
        }
        let input = Input::Interleaved {
            samples: input,
            channels: self.channels,
        };
        self.run(&input, &mut Output::Interleaved(output));
        Ok(())
    }

    /// [`Self::process`] for one slice per channel, appending to one vector per channel.
    pub fn process_planar(&mut self, input: &[&[S]], output: &mut [Vec<S>]) -> Result<()> {
        self.check_planar(input.len())?;
        self.check_planar(output.len())?;
        if input.iter().any(|c| c.len() != input[0].len()) {
            return Err(Error::UnequalLengths);
        }
        self.run(&Input::Planar(input), &mut Output::Planar(output));
        Ok(())
    }

    /// Ends the stream: appends the remaining frames so the total is [`output_len`] of the
    /// input, then starts afresh as [`Self::reset`] does.
    pub fn flush(&mut self, output: &mut Vec<S>) {
        self.finish(&mut Output::Interleaved(output));
    }

    /// [`Self::flush`] into one vector per channel.
    pub fn flush_planar(&mut self, output: &mut [Vec<S>]) -> Result<()> {
        self.check_planar(output.len())?;
        self.finish(&mut Output::Planar(output));
        Ok(())
    }

    /// Forgets the stream so far, as if just constructed; keeps every allocation.
    pub fn reset(&mut self) {
        match &mut self.engine {
            Engine::Pass => {}
            Engine::Gather(g) => g.reset(),
            Engine::Scatter(s) => s.reset(),
        }
    }

    fn check_planar(&self, got: usize) -> Result<()> {
        if got == self.channels {
            Ok(())
        } else {
            Err(Error::ChannelMismatch {
                expected: self.channels,
                got,
            })
        }
    }

    fn run(&mut self, input: &Input<'_, S>, output: &mut Output<'_, S>) {
        match &mut self.engine {
            Engine::Pass => output.copy_from(input),
            Engine::Gather(g) => g.process(input, output),
            Engine::Scatter(s) => s.process(input, output),
        }
    }

    fn finish(&mut self, output: &mut Output<'_, S>) {
        match &mut self.engine {
            Engine::Pass => {}
            Engine::Gather(g) => g.flush(output),
            Engine::Scatter(s) => s.flush(output),
        }
    }
}
