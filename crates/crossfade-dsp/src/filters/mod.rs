//! The filters, one module each, and the one place a planned filter becomes a running one.
//!
//! A plan is what the chain runs: frame positions and linear gains at the filter's own input
//! rate, worked out from the user's words by `crossfade-graph` (which owns units and the job
//! format). Adding a filter: write its module (plan and filter), then make its arm in
//! [`build`] construct it.

pub mod fade;
pub mod gain;
pub mod mix;
pub mod normalize;
pub mod resample;
pub mod trim;

use crossfade_core::{AudioFrame, AudioSpec, ChannelLayout, Error, Filter};
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum FilterPlan {
    Gain(gain::GainPlan),
    Trim(trim::TrimPlan),
    Fade(fade::FadePlan),
    Normalize(normalize::NormalizePlan),
    Resample(resample::ResamplePlan),
    Mix(mix::MixPlan),
}

/// A running filter for `plan`, taking `f64` frames of `input`.
pub fn build(plan: &FilterPlan, input: &AudioSpec) -> Result<Box<dyn Filter>, Error> {
    match plan {
        FilterPlan::Gain(p) => Ok(Box::new(gain::Gain::new(input, p))),
        FilterPlan::Trim(p) => Ok(Box::new(trim::Trim::new(input, p))),
        FilterPlan::Fade(p) => Ok(Box::new(fade::Fade::new(input, p)?)),
        FilterPlan::Normalize(_) => Err(not_built("normalize")),
        FilterPlan::Resample(p) => Ok(Box::new(resample::Resample::new(input, p)?)),
        FilterPlan::Mix(p) => Ok(Box::new(mix::Mix::new(input, p)?)),
    }
}

fn not_built(name: &str) -> Error {
    Error::Unsupported {
        what: format!("the {name} filter is not built yet"),
    }
}

impl FilterPlan {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Gain(_) => "gain",
            Self::Trim(_) => "trim",
            Self::Fade(_) => "fade",
            Self::Normalize(_) => "normalize",
            Self::Resample(_) => "resample",
            Self::Mix(_) => "mix",
        }
    }

    /// What the filter puts out for `input`; refuses a plan that cannot run on it.
    pub fn output_spec(&self, input: &AudioSpec) -> Result<AudioSpec, Error> {
        match self {
            Self::Resample(p) => AudioSpec::new(p.rate, input.format(), input.layout().clone()),
            Self::Mix(p) => {
                if let Some(m) = &p.matrix {
                    check_matrix(m, input.layout().channels(), &p.layout)?;
                }
                AudioSpec::new(input.sample_rate(), input.format(), p.layout.clone())
            }
            Self::Gain(_) | Self::Trim(_) | Self::Fade(_) | Self::Normalize(_) => Ok(input.clone()),
        }
    }

    /// Frames out for `frames` in, when that can be known before running.
    pub fn output_frames(&self, input: &AudioSpec, frames: Option<u64>) -> Option<u64> {
        match self {
            Self::Trim(p) => p.output_frames(frames),
            Self::Resample(p) => p.output_frames(input.sample_rate(), frames),
            Self::Gain(_) | Self::Fade(_) | Self::Normalize(_) | Self::Mix(_) => frames,
        }
    }
}

fn check_matrix(m: &[Vec<f64>], inputs: u16, layout: &ChannelLayout) -> Result<(), Error> {
    let bad = |what: String| Err(Error::Unsupported { what });
    let outputs = layout.channels();
    if m.len() != usize::from(outputs) {
        let name = layout_words(layout);
        let s = if outputs == 1 { "" } else { "s" };
        return bad(format!(
            "mix matrix has {} rows but {name} has {outputs} channel{s}",
            m.len()
        ));
    }
    if let Some(row) = m.iter().find(|r| r.len() != usize::from(inputs)) {
        return bad(format!(
            "mix matrix rows have {} coefficients but the input has {inputs} channels",
            row.len()
        ));
    }
    Ok(())
}

fn layout_words(layout: &ChannelLayout) -> String {
    layout
        .name()
        .map_or_else(|| format!("{} channels", layout.channels()), str::to_owned)
}

/// Plain words for plans and snapshots, not a syntax.
impl fmt::Display for FilterPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gain(p) => write!(f, "gain x{}", p.linear),
            Self::Trim(p) => match p.end {
                Some(e) => write!(f, "trim frames {}..{e}", p.start),
                None => write!(f, "trim frames {}..end", p.start),
            },
            Self::Fade(p) => write!(
                f,
                "fade in {} frames, out {} frames, {}",
                p.fade_in,
                p.fade_out,
                p.curve.name()
            ),
            Self::Normalize(p) => write!(
                f,
                "normalize to {} LUFS, true peak at most {} dBTP (gain from a measuring pass)",
                p.target_lufs, p.ceiling_dbtp
            ),
            Self::Resample(p) => write!(f, "resample to {} Hz ({})", p.rate, p.quality.name()),
            Self::Mix(p) => {
                write!(f, "mix to {}", layout_words(&p.layout))?;
                match &p.matrix {
                    Some(m) => write!(f, " by matrix {m:?}"),
                    None => Ok(()),
                }
            }
        }
    }
}

/// Filters run on `f64`; the chain converts before the first one.
fn not_working_format(filter: &str, frame: &AudioFrame) -> Error {
    Error::Unsupported {
        what: format!(
            "the {filter} filter takes f64 frames, not {}",
            frame.samples.format().name()
        ),
    }
}
