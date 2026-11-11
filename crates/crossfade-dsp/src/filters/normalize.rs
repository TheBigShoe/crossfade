//! Loudness normalisation: one gain for the whole programme, from a measuring pass over the
//! signal as it reaches this filter (so whatever runs before it is measured too).
//!
//! The chain never runs this plan as it is: the pipeline measures first, then replaces it
//! with the [`GainPlan`](super::gain::GainPlan) the measurement gives. Not built yet.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalizePlan {
    /// Integrated loudness to reach, LUFS.
    pub target_lufs: f64,
    /// The true peak may not pass this, dBTP; it lowers the gain when it must.
    pub ceiling_dbtp: f64,
}
