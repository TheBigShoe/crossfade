//! What an encoder's analysis told its last frame, for comparisons with libopus.

use crate::Encoder;

/// `AnalysisInfo` as the last frame saw it, and the voice estimate (Q7) of its packet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnalysisProbe {
    pub valid: bool,
    pub tonality: f32,
    pub tonality_slope: f32,
    pub noisiness: f32,
    pub activity: f32,
    pub music_prob: f32,
    pub music_prob_min: f32,
    pub music_prob_max: f32,
    pub bandwidth: i32,
    pub activity_probability: f32,
    pub max_pitch_ratio: f32,
    pub leak_boost: [u8; 19],
    pub voice_est: i32,
}

/// The analysis behind `enc`'s last packet (its last frame).
pub fn last_analysis(enc: &Encoder) -> AnalysisProbe {
    let a = &enc.last_analysis;
    AnalysisProbe {
        valid: a.valid,
        tonality: a.tonality,
        tonality_slope: a.tonality_slope,
        noisiness: a.noisiness,
        activity: a.activity,
        music_prob: a.music_prob,
        music_prob_min: a.music_prob_min,
        music_prob_max: a.music_prob_max,
        bandwidth: a.bandwidth,
        activity_probability: a.activity_probability,
        max_pitch_ratio: a.max_pitch_ratio,
        leak_boost: a.leak_boost,
        voice_est: enc.last_voice_est,
    }
}
