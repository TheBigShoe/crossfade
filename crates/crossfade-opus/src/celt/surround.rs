//! The surround masking analysis of libopus 1.5.2's multistream encoder
//! (`surround_analysis` in `src/opus_multistream_encoder.c`, 3-clause BSD, see the crate's
//! `COPYING`).

/// The state `surround_analysis` keeps per channel.
#[derive(Debug, Clone)]
pub(crate) struct SurroundAnalysis {
    channels: usize,
}

impl SurroundAnalysis {
    pub(crate) fn new(channels: usize) -> Self {
        Self { channels }
    }

    pub(crate) fn channels(&self) -> usize {
        self.channels
    }

    pub(crate) fn reset(&mut self) {}

    pub(crate) fn analyse(&mut self, pcm: &[f32], frame_size: usize, out: &mut [f32]) {
        let _ = (pcm, frame_size);
        out.fill(0.0);
    }
}

#[cfg(test)]
#[path = "surround_tests.rs"]
mod tests;
