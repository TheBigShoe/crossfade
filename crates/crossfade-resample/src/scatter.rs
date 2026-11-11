//! The transposed form, for decimation by more than [`crate::MAX_GATHER_DECIMATION`]: each
//! input frame is spread over the outputs its kernel reaches, which accumulate until no later
//! input can reach them. Memory is the kernel's width in *output* samples, however large the
//! ratio, where the direct form would hold the kernel's width in input samples.

use crate::Sample;
use crate::design::{self, Kernel};
use crate::io::{Input, Output};
use crate::kernel;
use crate::quality::Params;
use crate::table::{self, Norm, Table};

/// Input frames between emitting finished outputs; also bounds the accumulators.
const BLOCK: usize = 1024;

#[derive(Debug, Clone)]
pub(crate) struct Scatter<S> {
    /// Output instants per `down` input frames (the reduced ratio, `up < down`).
    up: u64,
    down: u64,
    taps: usize,
    /// Coefficients for an input `q / down` of an output frame after an output instant.
    table: Table<S>,
    /// One buffer per channel; `acc[c][0]` is output frame `start`.
    acc: Vec<Vec<S>>,
    /// With interpolated phases, the sum of the coefficients each output has received (the
    /// same for every channel): dividing by it makes the DC gain exactly 1 where the
    /// integral's normalisation leaves ~1e-11. Empty for an exact table.
    weights: Vec<S>,
    start: i64,
    /// The next input's instant in output frames: `at` plus `phase / down`.
    at: u64,
    phase: u64,
    produced: u64,
    received: u64,
}

impl<S: Sample> Scatter<S> {
    pub(crate) fn new(params: &Params, up: u64, down: u64, channels: usize) -> Self {
        // Designed at the output (lower) rate.
        let taps = design::taps(params, 1.0);
        let kernel = Kernel::new(params, 1.0, (taps / 2) as f64);
        let table = Table::new(
            &kernel,
            taps,
            down,
            params.rows_per_sample,
            |rows, per| match per {
                None => Norm::Classes(up),
                // Each output sums the kernel at a spacing of up / down output frames.
                Some(per) => {
                    Norm::Scale(up as f64 / (down as f64 * table::integral(rows, taps, per)))
                }
            },
        );
        let len = taps + BLOCK + 2;
        let weights = if table.is_exact() {
            Vec::new()
        } else {
            vec![S::ZERO; len]
        };
        let mut s = Self {
            up,
            down,
            taps,
            table,
            acc: vec![vec![S::ZERO; len]; channels],
            weights,
            start: 0,
            at: 0,
            phase: 0,
            produced: 0,
            received: 0,
        };
        s.reset();
        s
    }

    fn half(&self) -> usize {
        self.taps / 2
    }

    pub(crate) fn memory(&self) -> usize {
        self.table.len() + self.weights.len() + self.acc.iter().map(Vec::len).sum::<usize>()
    }

    /// Output 0 is complete once an input lands `half` output frames past it.
    pub(crate) fn latency(&self) -> u64 {
        crate::ceil_ratio(self.half() as u64, self.down, self.up) - 1
    }

    pub(crate) fn reset(&mut self) {
        for a in &mut self.acc {
            a.fill(S::ZERO);
        }
        self.weights.fill(S::ZERO);
        // Input 0 reaches back to output 1 - half.
        self.start = 1 - self.half() as i64;
        self.at = 0;
        self.phase = 0;
        self.produced = 0;
        self.received = 0;
    }

    pub(crate) fn process(&mut self, input: &Input<'_, S>, output: &mut Output<'_, S>) {
        let half = self.half() as i64;
        let frames = input.frames();
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(BLOCK);
            for f in done..done + n {
                let coefs = self.table.coefs(self.phase, self.down);
                let from = (self.at as i64 - half + 1 - self.start) as usize;
                for (ch, acc) in self.acc.iter_mut().enumerate() {
                    kernel::axpy(&mut acc[from..from + self.taps], input.get(f, ch), coefs);
                }
                if let Some(w) = self.weights.get_mut(from..from + self.taps) {
                    kernel::axpy(w, S::ONE, coefs);
                }
                self.phase += self.up;
                if self.phase >= self.down {
                    self.phase -= self.down;
                    self.at += 1;
                }
            }
            self.received += n as u64;
            done += n;
            // No later input reaches below its own instant minus half + 1.
            let complete = (self.at as i64 - half + 1).max(0) as u64;
            self.emit(output, complete, false);
        }
    }

    pub(crate) fn flush(&mut self, output: &mut Output<'_, S>) {
        let target = crate::ceil_ratio(self.received, self.up, self.down);
        self.emit(output, target, true);
        self.reset();
    }

    /// Writes outputs up to (not including) `end`, then drops them from the accumulators.
    /// `flushing`: the stream has ended, so the last outputs lack the inputs after it.
    fn emit(&mut self, output: &mut Output<'_, S>, end: u64, flushing: bool) {
        if end <= self.produced {
            return;
        }
        let from = (self.produced as i64 - self.start) as usize;
        let count = (end - self.produced) as usize;
        let half = self.half() as u64;
        output.reserve(count, self.acc.len());
        for (n, k) in (self.produced..end).zip(from..) {
            // Outputs within the kernel's reach of either end of the stream are missing the
            // (zero) inputs beyond it, and their weight with them: those keep the integral's
            // normalisation, as the zero padding they stand for asks.
            let whole = n >= half
                && (!flushing
                    || u128::from(n + half) * u128::from(self.down)
                        <= u128::from(self.received) * u128::from(self.up));
            match self.weights.get(k).filter(|_| whole) {
                Some(&w) => {
                    for (ch, acc) in self.acc.iter().enumerate() {
                        output.push(ch, acc[k] / w);
                    }
                }
                None => {
                    for (ch, acc) in self.acc.iter().enumerate() {
                        output.push(ch, acc[k]);
                    }
                }
            }
        }
        self.produced = end;
        let drop = (end as i64 - self.start) as usize;
        for acc in &mut self.acc {
            acc.copy_within(drop.., 0);
            let len = acc.len();
            acc[len - drop..].fill(S::ZERO);
        }
        if !self.weights.is_empty() {
            self.weights.copy_within(drop.., 0);
            let len = self.weights.len();
            self.weights[len - drop..].fill(S::ZERO);
        }
        self.start = end as i64;
    }
}
