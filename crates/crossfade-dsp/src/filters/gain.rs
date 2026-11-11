//! A fixed gain: every sample times one factor.

use crossfade_core::{AudioFrame, AudioSpec, Error, Filter, Samples};

/// The gain as the chain runs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainPlan {
    /// Amplitude factor (see [`crate::level::db_to_linear`]).
    pub linear: f64,
}

pub struct Gain {
    spec: AudioSpec,
    linear: f64,
    buf: Vec<f64>,
    pts: i64,
    ready: bool,
}

impl Gain {
    pub fn new(input: &AudioSpec, plan: &GainPlan) -> Self {
        Self {
            spec: input.clone(),
            linear: plan.linear,
            buf: Vec::new(),
            pts: 0,
            ready: false,
        }
    }
}

impl Filter for Gain {
    fn output_spec(&self) -> &AudioSpec {
        &self.spec
    }

    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        let Samples::F64(s) = &frame.samples else {
            return Err(super::not_working_format("gain", frame));
        };
        let k = self.linear;
        self.buf.clear();
        self.buf.extend(s.iter().map(|&x| x * k));
        self.pts = frame.pts;
        self.ready = !self.buf.is_empty();
        Ok(())
    }

    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        if !std::mem::take(&mut self.ready) {
            return Ok(false);
        }
        out.sample_rate = self.spec.sample_rate();
        out.layout.clone_from(self.spec.layout());
        out.pts = self.pts;
        // Swap buffers so neither side allocates in steady state.
        let mut spare = match std::mem::replace(&mut out.samples, Samples::F64(Vec::new())) {
            Samples::F64(v) => v,
            _ => Vec::new(),
        };
        std::mem::swap(&mut spare, &mut self.buf);
        out.samples = Samples::F64(spare);
        Ok(true)
    }

    /// Holds nothing back.
    fn drain(&mut self) {}
}
