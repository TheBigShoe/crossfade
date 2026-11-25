//! The analyser's small recurrent network (libopus 1.5.2's `src/mlp.c`, 3-clause BSD, see the
//! crate's `COPYING`): a dense layer of 32 `tansig` units over the 25 features, a GRU of 24,
//! and two `sigmoid` outputs, the probabilities of music and of activity.

use super::mlp_data::{
    LAYER0_BIAS, LAYER0_WEIGHTS, LAYER1_BIAS, LAYER1_RECUR_WEIGHTS, LAYER1_WEIGHTS, LAYER2_BIAS,
    LAYER2_WEIGHTS,
};

const WEIGHTS_SCALE: f32 = 1.0 / 128.0;
const INPUTS: usize = 25;
const DENSE: usize = 32;
const GRU: usize = 24;

/// The GRU's state between frames; `run` once per analysis frame.
#[derive(Debug, Clone, Default)]
pub(super) struct Mlp {
    state: [f32; GRU],
}

/// `tansig_approx`: a rational approximation of `tanh`, clamped to [-1, 1].
#[allow(clippy::excessive_precision)] // the reference's literals
fn tansig(x: f32) -> f32 {
    const N0: f32 = 952.528_015_14;
    const N1: f32 = 96.392_356_87;
    const N2: f32 = 0.608_630_42;
    const D0: f32 = 952.723_999_02;
    const D1: f32 = 413.368_011_47;
    const D2: f32 = 11.886_009_22;
    let x2 = x * x;
    let num = (N2 * x2 + N1) * x2 + N0;
    let den = (D2 * x2 + D1) * x2 + D0;
    let num = num * x / den;
    // The reference's MAX32(-1, MIN32(1, num)).
    let num = if 1.0 < num { 1.0 } else { num };
    if -1.0 > num { -1.0 } else { num }
}

fn sigmoid(x: f32) -> f32 {
    0.5 + 0.5 * tansig(0.5 * x)
}

/// `gemm_accum`: `out[i] += sum_j weights[j * stride + i] * x[j]`, in the reference's order.
fn gemm_accum(out: &mut [f32], weights: &[i8], stride: usize, x: &[f32]) {
    for (i, o) in out.iter_mut().enumerate() {
        for (j, &v) in x.iter().enumerate() {
            *o += f32::from(weights[j * stride + i]) * v;
        }
    }
}

impl Mlp {
    /// Forgets the recurrent state (the analyser starts afresh whole).
    #[cfg(test)]
    pub(super) fn reset(&mut self) {
        self.state = [0.0; GRU];
    }

    /// The music and activity probabilities for one frame's features.
    pub(super) fn run(&mut self, features: &[f32; INPUTS]) -> [f32; 2] {
        let mut dense = [0.0f32; DENSE];
        for (o, &b) in dense.iter_mut().zip(&LAYER0_BIAS) {
            *o = f32::from(b);
        }
        gemm_accum(&mut dense, &LAYER0_WEIGHTS, DENSE, features);
        for o in &mut dense {
            *o = tansig(*o * WEIGHTS_SCALE);
        }
        self.gru(&dense);
        let mut out = [0.0f32; 2];
        for (o, &b) in out.iter_mut().zip(&LAYER2_BIAS) {
            *o = f32::from(b);
        }
        gemm_accum(&mut out, &LAYER2_WEIGHTS, 2, &self.state);
        out.map(|o| sigmoid(o * WEIGHTS_SCALE))
    }

    /// `analysis_compute_gru`: the update and reset gates, then the new state.
    fn gru(&mut self, input: &[f32; DENSE]) {
        let stride = 3 * GRU;
        let gate = |k: usize, state: &[f32; GRU]| {
            let mut g = [0.0f32; GRU];
            for (o, &b) in g.iter_mut().zip(&LAYER1_BIAS[k * GRU..]) {
                *o = f32::from(b);
            }
            gemm_accum(&mut g, &LAYER1_WEIGHTS[k * GRU..], stride, input);
            gemm_accum(&mut g, &LAYER1_RECUR_WEIGHTS[k * GRU..], stride, state);
            g
        };
        let z = gate(0, &self.state).map(|v| sigmoid(WEIGHTS_SCALE * v));
        let r = gate(1, &self.state).map(|v| sigmoid(WEIGHTS_SCALE * v));
        let mut reset_state = [0.0f32; GRU];
        for i in 0..GRU {
            reset_state[i] = self.state[i] * r[i];
        }
        let h = gate(2, &reset_state);
        for i in 0..GRU {
            self.state[i] = z[i] * self.state[i] + (1.0 - z[i]) * tansig(WEIGHTS_SCALE * h[i]);
        }
    }
}

#[cfg(test)]
#[path = "mlp_tests.rs"]
mod tests;
