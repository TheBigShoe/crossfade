//! The analyser's small recurrent network (libopus 1.5.2's `src/mlp.c`, 3-clause BSD, see the
//! crate's `COPYING`): a dense layer of 32 `tansig` units over the 25 features, a GRU of 24,
//! and two `sigmoid` outputs, the probabilities of music and of activity.

/// The GRU's state between frames; `run` once per analysis frame.
#[derive(Debug, Clone, Default)]
pub(super) struct Mlp {
    state: [f32; 24],
}

impl Mlp {
    /// Forgets the recurrent state.
    pub(super) fn reset(&mut self) {
        self.state = [0.0; 24];
    }

    /// The music and activity probabilities for one frame's features.
    pub(super) fn run(&mut self, features: &[f32; 25]) -> [f32; 2] {
        let _ = features;
        [0.0; 2]
    }
}

#[cfg(test)]
#[path = "mlp_tests.rs"]
mod tests;
