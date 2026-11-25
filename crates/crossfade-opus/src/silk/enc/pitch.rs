//! The pitch estimator (libopus `silk/fixed/pitch_analysis_core_FIX.c`, with
//! `resampler_down2.c` and `resampler_down2_3.c`): a coarse search at 4 kHz, a refinement
//! with the contour codebook at 8 kHz, and at 12 and 16 kHz a last search on the signal itself.

#[cfg(test)]
#[path = "pitch_tests.rs"]
mod tests;
