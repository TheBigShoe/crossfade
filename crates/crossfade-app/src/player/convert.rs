//! The stream's frames to the device's: interleaved `f32` at its rate and channel count.

use super::OutputFormat;
use crossfade_core::{AudioFrame, AudioSpec, ChannelLayout, Error, Filter, SampleFormat, Samples};
use crossfade_dsp::chain::Chain;
use crossfade_dsp::convert::convert_format;
use crossfade_dsp::dither::Dither;
use crossfade_dsp::filters::mix::{Mix, MixPlan, standard_matrix};
use crossfade_dsp::filters::resample::{Quality, Resample, ResamplePlan};

/// The layout the player takes a device with `channels` outputs to have: FLAC's default
/// for the count (stereo for two), unpositioned past eight.
pub fn device_layout(channels: u16) -> Result<ChannelLayout, Error> {
    ChannelLayout::default_for(channels)
}

/// The standard mix when there is one; otherwise each input to the output of the same index
/// (mono to the first two), the rest silent. A preview never refuses a layout.
fn matrix(from: &ChannelLayout, to: &ChannelLayout) -> Vec<Vec<f64>> {
    standard_matrix(from, to).unwrap_or_else(|_| {
        let (n_in, n_out) = (usize::from(from.channels()), usize::from(to.channels()));
        let mut m = vec![vec![0.0; n_in]; n_out];
        for (o, row) in m.iter_mut().enumerate() {
            if o < n_in {
                row[o] = 1.0;
            } else if n_in == 1 && o == 1 {
                row[0] = 1.0;
            }
        }
        m
    })
}

pub(crate) struct Converter {
    /// `None` when only the sample format changes.
    chain: Option<Chain>,
    out: AudioFrame,
}

impl Converter {
    pub(crate) fn new(input: &AudioSpec, to: OutputFormat) -> Result<Self, Error> {
        let layout = device_layout(to.channels)?;
        let mut spec = AudioSpec::new(
            input.sample_rate(),
            SampleFormat::F64,
            input.layout().clone(),
        )?;
        let mut stages: Vec<Box<dyn Filter>> = Vec::new();
        let mix_first = to.channels < input.layout().channels();
        let mix = |spec: &mut AudioSpec, stages: &mut Vec<Box<dyn Filter>>| {
            if spec.layout() == &layout {
                return Ok::<(), Error>(());
            }
            let plan = MixPlan {
                layout: layout.clone(),
                matrix: Some(matrix(spec.layout(), &layout)),
            };
            let stage = Mix::new(spec, &plan)?;
            *spec = stage.output_spec().clone();
            stages.push(Box::new(stage));
            Ok(())
        };
        if mix_first {
            mix(&mut spec, &mut stages)?;
        }
        if spec.sample_rate() != to.sample_rate {
            let plan = ResamplePlan {
                rate: to.sample_rate,
                // Preview: the cheapest preset is far above what a listener checking a file
                // will notice, and costs least on the worker.
                quality: Quality::Fast,
            };
            let stage = Resample::new(&spec, &plan)?;
            spec = stage.output_spec().clone();
            stages.push(Box::new(stage));
        }
        if !mix_first {
            mix(&mut spec, &mut stages)?;
        }
        let chain = if stages.is_empty() {
            None
        } else {
            Some(Chain::with_filters(
                input,
                stages,
                SampleFormat::F32,
                Dither::None,
            )?)
        };
        let out = AudioSpec::new(to.sample_rate, SampleFormat::F32, layout)?;
        Ok(Self {
            chain,
            out: AudioFrame::new(&out),
        })
    }

    /// Converts `frame`, appending what comes out (the resampler holds some back until
    /// [`drain`](Self::drain)).
    pub(crate) fn push(&mut self, frame: &AudioFrame, out: &mut Vec<f32>) -> Result<(), Error> {
        match &mut self.chain {
            None => {
                convert_format(frame, SampleFormat::F32, &mut self.out)?;
                append(&self.out, out);
            }
            Some(chain) => {
                chain.send_frame(frame)?;
                while chain.receive_frame(&mut self.out)? {
                    append(&self.out, out);
                }
            }
        }
        Ok(())
    }

    /// The end of the input: whatever is held back.
    pub(crate) fn drain(&mut self, out: &mut Vec<f32>) -> Result<(), Error> {
        if let Some(chain) = &mut self.chain {
            chain.drain();
            while chain.receive_frame(&mut self.out)? {
                append(&self.out, out);
            }
        }
        Ok(())
    }
}

fn append(frame: &AudioFrame, out: &mut Vec<f32>) {
    if let Samples::F32(s) = &frame.samples {
        out.extend_from_slice(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unmixable_layout_still_plays_its_channels_in_order() {
        // Nine unlabelled channels have no default layout, so no standard mix.
        let from = ChannelLayout::unpositioned(9).unwrap();
        let m = matrix(&from, &ChannelLayout::stereo());
        let mut expected = vec![vec![0.0; 9]; 2];
        expected[0][0] = 1.0;
        expected[1][1] = 1.0;
        assert_eq!(m, expected);
    }
}
