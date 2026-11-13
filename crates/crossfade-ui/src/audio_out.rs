//! The system's default output device as an [`AudioSink`], through `cpal`.
//!
//! The device is opened only by the app binary's wiring ([`default_output`]); tests never
//! open one. They exercise [`Glue`], the part of the callback that is ours, against a fake
//! stream. Format: `f32` at the device's default rate and channel count when the device offers
//! it there, otherwise the default format, converted in the callback through a buffer
//! allocated once.

use crossfade_app::player::{AudioSink, Feed, OutputFormat};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};

/// Frames converted at a time when the device does not take `f32`; a bigger callback is
/// served in several passes.
const SCRATCH_FRAMES: usize = 4096;

/// What the sink's callback runs: pulls `f32` from `pull` and hands it over in the device's
/// sample type. Never allocates after construction.
pub struct Glue<P> {
    pull: P,
    channels: usize,
    scratch: Vec<f32>,
}

impl<P: FnMut(&mut [f32])> Glue<P> {
    pub fn new(pull: P, channels: usize) -> Self {
        Self {
            pull,
            channels: channels.max(1),
            scratch: Vec::new(),
        }
    }

    pub fn fill_f32(&mut self, _: &mut [f32]) {}

    pub fn fill<T: SizedSample + FromSample<f32>>(&mut self, _: &mut [T]) {}

    /// Allocates the scratch buffer now, before the glue moves to the audio thread.
    pub fn with_scratch(mut self) -> Self {
        self.scratch = vec![0.0; SCRATCH_FRAMES * self.channels];
        self
    }
}

/// The default output device, its configuration chosen but no stream built yet.
pub struct DeviceSink {
    device: cpal::Device,
    config: cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    name: String,
    stream: Option<cpal::Stream>,
}

impl DeviceSink {
    /// What the device is and runs at, for a log line.
    pub fn describe(&self) -> String {
        format!(
            "{}, {} Hz, {} channels, {}",
            self.name, self.config.sample_rate, self.config.channels, self.sample_format
        )
    }

    fn build<T>(&self, feed: Feed) -> Result<cpal::Stream, cpal::Error>
    where
        T: SizedSample + FromSample<f32>,
    {
        let channels = usize::from(self.config.channels);
        let mut feed = feed;
        let error = |e: cpal::Error| eprintln!("crossfade: audio output: {e}");
        if self.sample_format == cpal::SampleFormat::F32 {
            let mut glue = Glue::new(move |out: &mut [f32]| feed.fill(out), channels);
            return self.device.build_output_stream::<f32, _, _>(
                self.config.clone(),
                move |out: &mut [f32], _| glue.fill_f32(out),
                error,
                None,
            );
        }
        let mut glue = Glue::new(move |out: &mut [f32]| feed.fill(out), channels).with_scratch();
        self.device.build_output_stream::<T, _, _>(
            self.config.clone(),
            move |out: &mut [T], _| glue.fill(out),
            error,
            None,
        )
    }
}

/// The system's default output, or why there is none (playback is then off).
pub fn default_output() -> Result<DeviceSink, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no audio output device".to_owned())?;
    let default = device
        .default_output_config()
        .map_err(|e| format!("the audio output has no usable format: {e}"))?;
    let (rate, channels) = (default.sample_rate(), default.channels());
    // `f32` at the default rate and channels when the device offers it there.
    let float = device.supported_output_configs().ok().and_then(|mut all| {
        all.find(|c| {
            c.sample_format() == cpal::SampleFormat::F32
                && c.channels() == channels
                && (c.min_sample_rate()..=c.max_sample_rate()).contains(&rate)
        })
    });
    let chosen = match float {
        Some(range) => range.with_sample_rate(rate),
        None => default,
    };
    let name = device
        .description()
        .map(|d| d.to_string())
        .unwrap_or_else(|_| "default output".to_owned());
    Ok(DeviceSink {
        config: chosen.config(),
        sample_format: chosen.sample_format(),
        device,
        name,
        stream: None,
    })
}

impl AudioSink for DeviceSink {
    fn format(&self) -> OutputFormat {
        OutputFormat {
            sample_rate: self.config.sample_rate,
            channels: self.config.channels,
        }
    }

    fn connect(&mut self, feed: Feed) -> Result<(), String> {
        use cpal::SampleFormat as F;
        let stream = match self.sample_format {
            F::F32 => self.build::<f32>(feed),
            F::F64 => self.build::<f64>(feed),
            F::I16 => self.build::<i16>(feed),
            F::I32 => self.build::<i32>(feed),
            F::I8 => self.build::<i8>(feed),
            F::U8 => self.build::<u8>(feed),
            F::U16 => self.build::<u16>(feed),
            F::U32 => self.build::<u32>(feed),
            other => return Err(format!("the audio output wants {other} samples")),
        }
        .map_err(|e| format!("could not open the audio output: {e}"))?;
        // Built streams may start on their own on some hosts; start only when asked.
        let _ = stream.pause();
        self.stream = Some(stream);
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        match &self.stream {
            Some(s) => s.play().map_err(|e| e.to_string()),
            None => Err("the audio output is not open".into()),
        }
    }

    fn pause(&mut self) -> Result<(), String> {
        match &self.stream {
            Some(s) => s.pause().map_err(|e| e.to_string()),
            None => Ok(()),
        }
    }

    fn stop(&mut self) {
        self.stream = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::Sample;

    /// A ramp: sample `i` of the stream is `i / 1000`.
    fn ramp() -> impl FnMut(&mut [f32]) {
        let mut next = 0u32;
        move |out: &mut [f32]| {
            for s in out {
                *s = next as f32 / 1000.0;
                next += 1;
            }
        }
    }

    #[test]
    fn float_devices_get_the_feed_straight_through() {
        let mut glue = Glue::new(ramp(), 2);
        let mut out = [9.0f32; 6];
        glue.fill_f32(&mut out);
        assert_eq!(out, [0.0, 0.001, 0.002, 0.003, 0.004, 0.005]);
    }

    #[test]
    fn a_partial_frame_at_the_end_of_a_buffer_is_silent_and_skips_nothing() {
        let mut glue = Glue::new(ramp(), 2);
        let mut out = [9.0f32; 5];
        glue.fill_f32(&mut out);
        assert_eq!(out, [0.0, 0.001, 0.002, 0.003, 0.0]);
        let mut next = [9.0f32; 2];
        glue.fill_f32(&mut next);
        assert_eq!(next, [0.004, 0.005]);
    }

    #[test]
    fn integer_devices_are_converted_in_scratch_sized_passes_in_order() {
        let mut glue = Glue::new(
            {
                let mut value = 0i32;
                move |out: &mut [f32]| {
                    for s in out {
                        *s = (value % 2) as f32 * 0.5;
                        value += 1;
                    }
                }
            },
            2,
        )
        .with_scratch();
        let frames = SCRATCH_FRAMES * 2 + 3;
        let mut out = vec![1i16; frames * 2];
        glue.fill(&mut out);
        for (i, s) in out.iter().enumerate() {
            let want = if i % 2 == 1 { i16::from_sample(0.5f32) } else { 0 };
            assert_eq!(*s, want, "sample {i}");
        }
    }

    #[test]
    fn unsigned_silence_is_the_midpoint() {
        let mut glue = Glue::new(|out: &mut [f32]| out.fill(0.0), 1);
        let mut out = [0u8; 4];
        glue.fill(&mut out);
        assert_eq!(out, [128; 4]);
    }
}
