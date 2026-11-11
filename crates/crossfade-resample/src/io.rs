//! The two layouts a caller can use, behind one interface the engines read and write.

use crate::Sample;

pub(crate) enum Input<'a, S> {
    Interleaved { samples: &'a [S], channels: usize },
    Planar(&'a [&'a [S]]),
}

impl<S: Sample> Input<'_, S> {
    pub(crate) fn frames(&self) -> usize {
        match self {
            Self::Interleaved { samples, channels } => samples.len() / channels,
            Self::Planar(chans) => chans.first().map_or(0, |c| c.len()),
        }
    }

    /// Copies `dst.len()` frames of channel `ch` from frame `start` on.
    pub(crate) fn copy_channel(&self, ch: usize, start: usize, dst: &mut [S]) {
        match self {
            Self::Interleaved { samples, channels } => {
                let src = samples[start * channels + ch..].iter().step_by(*channels);
                for (d, s) in dst.iter_mut().zip(src) {
                    *d = *s;
                }
            }
            Self::Planar(chans) => dst.copy_from_slice(&chans[ch][start..start + dst.len()]),
        }
    }

    pub(crate) fn get(&self, frame: usize, ch: usize) -> S {
        match self {
            Self::Interleaved { samples, channels } => samples[frame * channels + ch],
            Self::Planar(chans) => chans[ch][frame],
        }
    }
}

pub(crate) enum Output<'a, S> {
    Interleaved(&'a mut Vec<S>),
    Planar(&'a mut [Vec<S>]),
}

impl<S: Sample> Output<'_, S> {
    /// Room for `frames` more frames of `channels` channels.
    pub(crate) fn reserve(&mut self, frames: usize, channels: usize) {
        match self {
            Self::Interleaved(v) => v.reserve(frames.saturating_mul(channels)),
            Self::Planar(vs) => vs.iter_mut().for_each(|v| v.reserve(frames)),
        }
    }

    /// Appends one sample; for interleaved output the caller pushes a frame's channels in order.
    pub(crate) fn push(&mut self, ch: usize, value: S) {
        match self {
            Self::Interleaved(v) => v.push(value),
            Self::Planar(vs) => vs[ch].push(value),
        }
    }

    pub(crate) fn copy_from(&mut self, input: &Input<'_, S>) {
        match (self, input) {
            (Self::Interleaved(v), Input::Interleaved { samples, .. }) => {
                v.extend_from_slice(samples);
            }
            (Self::Planar(vs), Input::Planar(chans)) => {
                for (v, c) in vs.iter_mut().zip(chans.iter()) {
                    v.extend_from_slice(c);
                }
            }
            // The public calls pair interleaved with interleaved and planar with planar.
            _ => unreachable!("input and output layouts differ"),
        }
    }
}
