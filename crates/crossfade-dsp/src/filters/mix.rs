//! Channel mixing to a new layout: the standard down- and up-mixes, or a matrix.
//!
//! Not built yet: the running filter belongs here (see `build` in the parent module).

use crossfade_core::ChannelLayout;

#[derive(Debug, Clone, PartialEq)]
pub struct MixPlan {
    pub layout: ChannelLayout,
    /// `matrix[out][in]`, linear gains; `None` picks the standard mix for the two layouts.
    pub matrix: Option<Vec<Vec<f64>>>,
}
