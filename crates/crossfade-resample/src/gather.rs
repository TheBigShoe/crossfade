//! The direct form: each output is a dot product of one table row with the input history
//! around its instant. Used for upsampling and for downsampling by up to
//! [`crate::MAX_GATHER_DECIMATION`], where the history stays small.

use crate::Sample;
use crate::design::{self, Kernel};
use crate::io::{Input, Output};
use crate::kernel;
use crate::quality::Params;
use crate::table::{Norm, Table};

/// Input frames copied into the history per step; bounds the history to `taps + BLOCK`.
const BLOCK: usize = 1024;

#[derive(Debug, Clone)]
pub(crate) struct Gather<S> {
    /// Output instants per `down` input frames (the reduced ratio); also the phase count.
    up: u64,
    down: u64,
    taps: usize,
    /// The coefficients for an output `p / up` of a frame after an input frame.
    table: Table<S>,
    /// One buffer per channel; `hist[c][0]` is input frame `start` (negative frames are the
    /// zeros before the stream).
    hist: Vec<Vec<S>>,
    start: i64,
    len: usize,
    /// The next output's instant: input frame `at` plus `phase / up`.
    at: u64,
    phase: u64,
    produced: u64,
    received: u64,
}

impl<S: Sample> Gather<S> {
    pub(crate) fn new(params: &Params, up: u64, down: u64, channels: usize) -> Self {
        // Downsampling narrows the kernel's band by the ratio and widens it in time.
        let scale = if up < down {
            up as f64 / down as f64
        } else {
            1.0
        };
        let taps = design::taps(params, scale);
        let kernel = Kernel::new(params, scale, (taps / 2) as f64);
        // Rows per input sample when interpolating: the same resolution in kernel time.
        let per = (libm::ceil(params.rows_per_sample as f64 * scale) as u64).max(4);
        let table = Table::new(&kernel, taps, up, per, |_, _| Norm::Rows);
        let mut g = Self {
            up,
            down,
            taps,
            table,
            hist: vec![vec![S::ZERO; taps + BLOCK]; channels],
            start: 0,
            len: 0,
            at: 0,
            phase: 0,
            produced: 0,
            received: 0,
        };
        g.reset();
        g
    }

    pub(crate) fn memory(&self) -> usize {
        self.table.len() + self.hist.iter().map(Vec::len).sum::<usize>()
    }

    fn half(&self) -> usize {
        self.taps / 2
    }

    pub(crate) fn latency(&self) -> u64 {
        self.half() as u64
    }

    pub(crate) fn reset(&mut self) {
        let lead = self.half() - 1;
        for h in &mut self.hist {
            h[..lead].fill(S::ZERO);
        }
        self.start = -(lead as i64);
        self.len = lead;
        self.at = 0;
        self.phase = 0;
        self.produced = 0;
        self.received = 0;
    }

    pub(crate) fn process(&mut self, input: &Input<'_, S>, output: &mut Output<'_, S>) {
        let frames = input.frames();
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(self.room());
            for (ch, h) in self.hist.iter_mut().enumerate() {
                input.copy_channel(ch, done, &mut h[self.len..self.len + n]);
            }
            self.len += n;
            self.received += n as u64;
            done += n;
            self.emit(output, u64::MAX);
        }
    }

    pub(crate) fn flush(&mut self, output: &mut Output<'_, S>) {
        let target = crate::ceil_ratio(self.received, self.up, self.down);
        while self.produced < target {
            let n = self.room();
            for h in &mut self.hist {
                h[self.len..self.len + n].fill(S::ZERO);
            }
            self.len += n;
            self.emit(output, target);
        }
        self.reset();
    }

    fn room(&self) -> usize {
        (self.taps + BLOCK - self.len).min(BLOCK)
    }

    /// Writes every output whose taps are all in the history, up to output `limit`, then
    /// drops the history no later output needs.
    fn emit(&mut self, output: &mut Output<'_, S>, limit: u64) {
        let half = self.half() as i64;
        let end = self.start + self.len as i64;
        // Outputs whose integer instant is at most `last` have all their taps.
        let last = end - 1 - half;
        if last >= self.at as i64 {
            let span = u128::from((last - self.at as i64 + 1) as u64) * u128::from(self.up)
                - u128::from(self.phase);
            let count = span
                .div_ceil(u128::from(self.down))
                .min(u128::from(limit - self.produced)) as u64;
            output.reserve(count as usize, self.hist.len());
            for _ in 0..count {
                let row = self.table.coefs(self.phase, self.up);
                let from = (self.at as i64 - half + 1 - self.start) as usize;
                let taps = self.taps;
                for (pair, h) in self.hist.chunks(2).enumerate() {
                    if let [l, r] = h {
                        let (y0, y1) =
                            kernel::dot2(row, &l[from..from + taps], &r[from..from + taps]);
                        output.push(2 * pair, y0);
                        output.push(2 * pair + 1, y1);
                    } else {
                        output.push(2 * pair, kernel::dot(row, &h[0][from..from + taps]));
                    }
                }
                self.phase += self.down;
                self.at += self.phase / self.up;
                self.phase %= self.up;
            }
            self.produced += count;
        }
        let keep = self.at as i64 - half + 1;
        if keep > self.start {
            let drop = (keep - self.start) as usize;
            for h in &mut self.hist {
                h.copy_within(drop..self.len, 0);
            }
            self.start = keep;
            self.len -= drop;
        }
    }
}
