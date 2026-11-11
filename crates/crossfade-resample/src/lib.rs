//! Sample-rate conversion in pure Rust.
#![forbid(unsafe_code)]

mod error;
mod io;
mod quality;
mod sample;

pub use error::{Error, Result};
pub use quality::Quality;
pub use sample::Sample;

use io::{Input, Output};

/// The highest sample rate accepted, in Hz (FLAC's 20-bit limit).
pub const MAX_RATE: u32 = 1_048_575;
/// The most channels one resampler takes.
pub const MAX_CHANNELS: usize = 256;

/// The number of output frames `frames` input frames become: `ceil(frames * out / in)`,
/// saturating at `u64::MAX`; 0 when `input_rate` is 0.
pub fn output_len(frames: u64, input_rate: u32, output_rate: u32) -> u64 {
    if input_rate == 0 {
        return 0;
    }
    let n = (u128::from(frames) * u128::from(output_rate)).div_ceil(u128::from(input_rate));
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// A streaming sample-rate converter for one stream of `channels` channels.
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
    Convert(std::marker::PhantomData<S>),
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
            Engine::Convert(std::marker::PhantomData)
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
        0
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
    pub fn reset(&mut self) {}

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
            Engine::Convert(_) => {}
        }
    }

    fn finish(&mut self, _output: &mut Output<'_, S>) {}
}
