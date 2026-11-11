//! What a job that normalises needs: a measuring pass, whose "encoder" is a loudness meter,
//! and a converting pass whose `normalize` stage has become the gain the measurement gave.

use crossfade_core::{AudioFrame, AudioSpec, Encoder, Error, Muxer, Packet};
use crossfade_dsp::chain::ChainPlan;
use crossfade_dsp::filters::gain::GainPlan;
use crossfade_dsp::filters::normalize::{Measure, Measurements, NormalizePlan, Options};
use std::sync::{Arc, Mutex, PoisonError};

/// Which pass a pipeline runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Pass {
    /// Convert; a `normalize` stage takes this gain (`None`: the job must not normalise).
    Convert(Option<GainPlan>),
    /// Measure the signal as it reaches the `normalize` stage; write nothing.
    Measure,
}

/// The meter a measuring pass fills, and what to do with its reading.
pub(crate) struct Measuring {
    measure: Arc<Mutex<Option<Measure>>>,
    pub(crate) plan: NormalizePlan,
}

impl Measuring {
    pub(crate) fn finish(self) -> Measurements {
        let mut slot = self.measure.lock().unwrap_or_else(PoisonError::into_inner);
        slot.take().expect("one finish per pass").finish()
    }
}

/// An encoder that only meters the frames it is given.
struct MeasureEncoder(Arc<Mutex<Option<Measure>>>);

impl Encoder for MeasureEncoder {
    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        slot.as_mut().map_or(Ok(()), |m| m.push(frame))
    }

    fn receive_packet(&mut self, _: &mut Packet) -> Result<bool, Error> {
        Ok(false)
    }

    fn drain(&mut self) {}
}

/// A muxer for a pass that writes nothing.
pub(crate) struct NullMuxer;

impl Muxer for NullMuxer {
    fn write_packet(&mut self, _: &Packet) -> Result<(), Error> {
        Ok(())
    }

    fn finish(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

/// The encoder that meters, and its reading.
pub(crate) type Meter = (Box<dyn Encoder>, Measuring);

/// The chain `pass` runs instead of `chain` on `input`, and for a measuring pass the meter
/// that stands in for the encoder (it meters `f64` frames of the stage before `normalize`).
pub(crate) fn for_pass(
    chain: Option<ChainPlan>,
    input: &AudioSpec,
    pass: Pass,
) -> Result<(Option<ChainPlan>, Option<Meter>), Error> {
    let normalize = chain.as_ref().and_then(ChainPlan::normalize);
    match (pass, normalize) {
        (Pass::Convert(_), None) => Ok((chain, None)),
        (Pass::Convert(Some(gain)), Some(_)) => {
            Ok((chain.map(|c| c.with_measured_gain(gain)), None))
        }
        (Pass::Convert(None), Some(_)) => Err(Error::Unsupported {
            what: "this job normalises, so it needs a measuring pass first: run it with `run`"
                .into(),
        }),
        (Pass::Measure, None) => Err(Error::Unsupported {
            what: "a measuring pass needs a normalize stage".into(),
        }),
        (Pass::Measure, Some((_, plan))) => {
            let prefix = chain
                .as_ref()
                .and_then(ChainPlan::before_normalize)
                .expect("there is a normalize stage");
            let (spec, _) = prefix.output(input, None)?;
            let measure = Arc::new(Mutex::new(Some(Measure::new(&spec, Options::default())?)));
            let encoder: Box<dyn Encoder> = Box::new(MeasureEncoder(measure.clone()));
            Ok((Some(prefix), Some((encoder, Measuring { measure, plan }))))
        }
    }
}
