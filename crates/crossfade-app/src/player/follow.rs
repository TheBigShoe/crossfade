//! Whether the waveform follows the playhead.

use crate::waveview::WaveView;

#[derive(Debug, Clone)]
pub struct Follow;

impl Follow {
    pub fn new() -> Self {
        Self
    }

    pub fn is_following(&self) -> bool {
        false
    }

    pub fn user_moved(&mut self) {}

    pub fn resume(&mut self) {}

    pub fn keep_in_view(&self, _: &mut WaveView, _: u64) -> bool {
        false
    }
}
