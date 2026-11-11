//! The filter chain: frames of any sample format in, converted to the `f64` working format,
//! through the filters in order, then quantised to the output format (with dither and a
//! clip count, see [`crate::dither`]). The chain is itself a [`Filter`].
//!
//! Automatic conversions (the output's sample rate and channel layout) are ordinary filters
//! the planner appends at the end of the list; the chain does not tell them apart.

use crate::convert::convert_format;
use crate::dither::{Dither, Quantiser};
use crate::filters::{FilterPlan, build};
use crossfade_core::{AudioFrame, AudioSpec, Error, Filter, SampleFormat, Samples};
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq)]
pub struct ChainPlan {
    pub filters: Vec<FilterPlan>,
    pub dither: Dither,
}

impl ChainPlan {
    /// The spec (in the working format) and frame count after the last filter, for `frames`
    /// of `input`, without running anything.
    pub fn output(
        &self,
        input: &AudioSpec,
        frames: Option<u64>,
    ) -> Result<(AudioSpec, Option<u64>), Error> {
        let mut spec = working(input)?;
        let mut frames = frames;
        for f in &self.filters {
            frames = f.output_frames(&spec, frames);
            spec = f.output_spec(&spec)?;
        }
        Ok((spec, frames))
    }
}

fn working(spec: &AudioSpec) -> Result<AudioSpec, Error> {
    AudioSpec::new(spec.sample_rate(), SampleFormat::F64, spec.layout().clone())
}

/// Where finished frames wait for `receive_frame`.
struct Sink {
    spec: AudioSpec,
    quantiser: Quantiser,
    ready: VecDeque<AudioFrame>,
    spare: Vec<AudioFrame>,
}

impl Sink {
    fn emit(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        let Samples::F64(s) = &frame.samples else {
            return Err(Error::Unsupported {
                what: "a filter put out a frame that is not f64".into(),
            });
        };
        if s.is_empty() {
            return Ok(());
        }
        let mut out = self
            .spare
            .pop()
            .unwrap_or_else(|| AudioFrame::new(&self.spec));
        out.sample_rate = frame.sample_rate;
        out.layout.clone_from(&frame.layout);
        out.pts = frame.pts;
        self.quantiser.quantise(s, &mut out.samples);
        self.ready.push_back(out);
        Ok(())
    }
}

pub struct Chain {
    stages: Vec<Box<dyn Filter>>,
    /// `outs[i]` receives stage `i`'s frames.
    outs: Vec<AudioFrame>,
    working: AudioFrame,
    sink: Sink,
    /// An error met while draining, which has no way to return one; the next
    /// `receive_frame` reports it.
    failed: Option<Error>,
}

impl Chain {
    /// Builds every filter in `plan` for `input`; the output is the last filter's rate and
    /// layout in `format`. Fails if any filter cannot be built.
    pub fn new(input: &AudioSpec, plan: &ChainPlan, format: SampleFormat) -> Result<Self, Error> {
        let mut spec = working(input)?;
        let mut stages = Vec::with_capacity(plan.filters.len());
        for p in &plan.filters {
            let stage = build(p, &spec)?;
            spec = stage.output_spec().clone();
            stages.push(stage);
        }
        Self::with_filters(input, stages, format, plan.dither)
    }

    /// A chain of filters already built, each taking `f64` frames of the previous one's output
    /// spec (the first takes `input`'s rate and layout).
    pub fn with_filters(
        input: &AudioSpec,
        stages: Vec<Box<dyn Filter>>,
        format: SampleFormat,
        dither: Dither,
    ) -> Result<Self, Error> {
        let mut spec = working(input)?;
        let outs = stages
            .iter()
            .map(|stage| {
                spec = stage.output_spec().clone();
                AudioFrame::new(&spec)
            })
            .collect();
        let out_spec = AudioSpec::new(spec.sample_rate(), format, spec.layout().clone())?;
        Ok(Self {
            stages,
            outs,
            working: AudioFrame::new(&working(input)?),
            sink: Sink {
                spec: out_spec,
                quantiser: Quantiser::new(format, dither),
                ready: VecDeque::new(),
                spare: Vec::new(),
            },
            failed: None,
        })
    }

    /// Samples saturated in the output format so far (see [`Quantiser::clipped`]).
    pub fn clipped(&self) -> u64 {
        self.sink.quantiser.clipped()
    }

    /// Moves everything stage `i` has ready down the chain. The protocol wants a stage
    /// emptied before its next input, so each frame goes all the way down first.
    fn flow(&mut self, i: usize) -> Result<(), Error> {
        while self.stages[i].receive_frame(&mut self.outs[i])? {
            match self.stages.get_mut(i + 1) {
                Some(next) => {
                    next.send_frame(&self.outs[i])?;
                    self.flow(i + 1)?;
                }
                None => self.sink.emit(&self.outs[i])?,
            }
        }
        Ok(())
    }
}

impl Filter for Chain {
    fn output_spec(&self) -> &AudioSpec {
        &self.sink.spec
    }

    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        let frame = if frame.samples.format() == SampleFormat::F64 {
            frame
        } else {
            convert_format(frame, SampleFormat::F64, &mut self.working)?;
            &self.working
        };
        match self.stages.first_mut() {
            Some(first) => {
                first.send_frame(frame)?;
                self.flow(0)
            }
            None => self.sink.emit(frame),
        }
    }

    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        if let Some(e) = self.failed.take() {
            return Err(e);
        }
        let Some(mut frame) = self.sink.ready.pop_front() else {
            return Ok(false);
        };
        std::mem::swap(out, &mut frame);
        self.sink.spare.push(frame);
        Ok(true)
    }

    /// Once any stage ignores further input, so does the chain: nothing sent later can reach
    /// past that stage.
    fn ignores_further_input(&self) -> bool {
        self.stages.iter().any(|s| s.ignores_further_input())
    }

    fn drain(&mut self) {
        for i in 0..self.stages.len() {
            self.stages[i].drain();
            if let Err(e) = self.flow(i) {
                self.failed.get_or_insert(e);
                return;
            }
        }
    }
}
