//! Whether the waveform follows the playhead. While playing, a playhead that leaves the visible
//! range turns the page: the view moves so the playhead is at its left edge, at the user's own
//! zoom. Scrolling, panning or zooming by hand stops that until the next play or seek.

use crate::waveview::WaveView;

#[derive(Debug, Clone)]
pub struct Follow {
    on: bool,
}

impl Default for Follow {
    fn default() -> Self {
        Self::new()
    }
}

impl Follow {
    pub fn new() -> Self {
        Self { on: true }
    }

    pub fn is_following(&self) -> bool {
        self.on
    }

    /// The user moved the view themselves.
    pub fn user_moved(&mut self) {
        self.on = false;
    }

    /// Play or seek: follow again.
    pub fn resume(&mut self) {
        self.on = true;
    }

    /// Moves `view` when `position` is outside it and the view is following; `true` when it
    /// moved. A view that shows the whole file never moves.
    pub fn keep_in_view(&self, view: &mut WaveView, position: u64) -> bool {
        if !self.on || view.is_fit() {
            return false;
        }
        let (start, end) = view.visible();
        if (start..end).contains(&position) {
            return false;
        }
        let before = view.start();
        view.pan_px(view.x_of(position));
        view.start() != before
    }
}
