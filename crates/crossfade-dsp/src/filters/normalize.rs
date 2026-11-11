//! Loudness normalisation: one gain for the whole programme, from a measuring pass over the
//! signal as it reaches this filter (so whatever runs before it is measured too).
//!
//! The chain never runs this plan as it is: the pipeline measures first ([`Measure`]), asks
//! the plan for its gain ([`NormalizePlan::gain`]), and replaces the plan with the
//! [`GainPlan`] that gives. Building the plan itself is still refused.

use super::gain::GainPlan;
use crossfade_core::{AudioFrame, AudioSpec, Channel, ChannelLayout, Error, Samples};
use crossfade_loudness::{ChannelRole, Measurements, Meter, Options, normalisation_gain};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalizePlan {
    /// Integrated loudness to reach, LUFS.
    pub target_lufs: f64,
    /// The true peak may not pass this, dBTP; it lowers the gain when it must.
    pub ceiling_dbtp: f64,
}

/// The gain a measurement asks for, and whether the ceiling set it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalizeGain {
    pub gain: GainPlan,
    /// The true-peak ceiling, not the target, set the gain.
    pub limited_by_ceiling: bool,
}

impl NormalizePlan {
    /// The gain that takes `m` to the target without the true peak (the sample peak when the
    /// meter ran without true peak) passing the ceiling. A programme with no loudness
    /// (silence, under 400 ms, everything under -70 LUFS) gets unity: there is nothing to
    /// normalise, and a gain from noise-floor readings would only amplify it.
    pub fn gain(&self, m: &Measurements) -> NormalizeGain {
        let g = normalisation_gain(
            m.integrated,
            self.target_lufs,
            m.true_peak.unwrap_or(m.sample_peak),
            self.ceiling_dbtp,
        );
        NormalizeGain {
            gain: GainPlan { linear: g.linear },
            limited_by_ceiling: g.limited_by_true_peak,
        }
    }
}

/// The meter's role for each channel of `layout`, in storage order. Front left, right and
/// centre are themselves (a mono file's only channel is unweighted, counted once), the LFE is
/// left out of loudness, back and side channels are surrounds (so a 7.1's two pairs both
/// weigh 1.41, per BS.1770-4), any other position is unweighted. An unpositioned layout takes
/// the standard order up to six channels and is all unweighted past that.
pub fn roles_for(layout: &ChannelLayout) -> Vec<ChannelRole> {
    let n = usize::from(layout.channels());
    let Some(positions) = layout.positions() else {
        return ChannelRole::standard_layout(n)
            .map(<[_]>::to_vec)
            .unwrap_or_else(|| vec![ChannelRole::Unweighted; n]);
    };
    if n == 1 {
        return vec![ChannelRole::Unweighted];
    }
    positions
        .iter()
        .map(|p| match p {
            Channel::FrontLeft => ChannelRole::Left,
            Channel::FrontRight => ChannelRole::Right,
            Channel::FrontCenter => ChannelRole::Centre,
            Channel::Lfe => ChannelRole::Lfe,
            Channel::BackLeft | Channel::SideLeft => ChannelRole::LeftSurround,
            Channel::BackRight | Channel::SideRight => ChannelRole::RightSurround,
            _ => ChannelRole::Unweighted,
        })
        .collect()
}

/// A loudness and peak meter that takes engine frames. It keeps no samples: 16 bytes per
/// 100 ms of programme (the meter's gate and range histories), about 0.6 MB an hour.
pub struct Measure {
    meter: Meter,
}

impl Measure {
    /// A rate the meter cannot take (below 8 kHz or above 1,048,575 Hz) is `Unsupported`.
    pub fn new(spec: &AudioSpec, options: Options) -> Result<Self, Error> {
        let meter =
            Meter::new(spec.sample_rate(), &roles_for(spec.layout()), options).map_err(|e| {
                Error::Unsupported {
                    what: format!(
                        "loudness measurement at {} Hz with {} channels: {e}",
                        spec.sample_rate(),
                        spec.layout().channels()
                    ),
                }
            })?;
        Ok(Self { meter })
    }

    /// Frames of `f64` only, in the layout given to `new`.
    pub fn push(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        let Samples::F64(s) = &frame.samples else {
            return Err(super::not_working_format("normalize", frame));
        };
        self.meter.push(s);
        Ok(())
    }

    pub fn finish(self) -> Measurements {
        self.meter.finish()
    }
}
