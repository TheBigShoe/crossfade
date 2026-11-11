//! Trim: keep frames `start..end` of the stream, sample-accurate.
//!
//! Not built yet: the running filter belongs here (see `build` in the parent module).

/// Frame positions at the filter's input rate; `end` is exclusive, `None` runs to the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrimPlan {
    pub start: u64,
    pub end: Option<u64>,
}

impl TrimPlan {
    pub fn output_frames(&self, frames: Option<u64>) -> Option<u64> {
        let n = frames?;
        Some(self.end.map_or(n, |e| e.min(n)).saturating_sub(self.start))
    }
}
