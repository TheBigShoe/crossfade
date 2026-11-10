//! Chooses how to code one channel of one block: the cheapest subframe, by exact bit count.

use super::fixed;
use super::lpc::{self, MAX_ORDER};
use super::rice::{RicePlan, RiceSearch};

/// A sample type the analysis runs on: `i32` for every channel but the side channel of a
/// 32-bit stream, which needs `i64`.
pub(super) trait Sample: Copy + Into<i64> {
    /// The low 32 bits; exact whenever the value is known to fit.
    fn low32(self) -> i32;
    fn shr(self, by: u32) -> Self;
    fn pool(analyzer: &mut Analyzer) -> &mut Vec<Self>;
}

impl Sample for i32 {
    fn low32(self) -> i32 {
        self
    }
    fn shr(self, by: u32) -> Self {
        self >> by
    }
    fn pool(analyzer: &mut Analyzer) -> &mut Vec<Self> {
        &mut analyzer.pool32
    }
}

impl Sample for i64 {
    fn low32(self) -> i32 {
        self as i32
    }
    fn shr(self, by: u32) -> Self {
        self >> by
    }
    fn pool(analyzer: &mut Analyzer) -> &mut Vec<Self> {
        &mut analyzer.pool64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Constant,
    Verbatim,
    Fixed(u8),
    Lpc(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stereo {
    Off,
    /// Rank the four channels by a cheap estimate, then search only the winning pair.
    Adaptive,
    /// Search all four channels fully and keep the cheapest pair.
    Exhaustive,
}

/// What a level asks of the search.
#[derive(Clone, Copy, Debug)]
pub(super) struct Level {
    pub(super) block: usize,
    pub(super) stereo: Stereo,
    /// Largest Rice partition order tried.
    pub(super) rice_order: u32,
    /// How many fixed orders, best by estimate first, get an exact search.
    pub(super) fixed_tries: usize,
    /// Largest LPC order; 0 means fixed predictors only.
    pub(super) max_lpc: usize,
    /// How many LPC orders, best by estimate first, get an exact search.
    pub(super) lpc_tries: usize,
    /// Quantised coefficient precision in bits, sign included.
    pub(super) precision: u32,
}

impl Level {
    pub(super) fn new(level: u8) -> Self {
        Self {
            block: if level <= 2 { 1152 } else { 4096 },
            stereo: match level {
                0 => Stereo::Off,
                1 | 3 | 4 => Stereo::Adaptive,
                _ => Stereo::Exhaustive,
            },
            rice_order: match level {
                0..=2 => 3,
                3 | 4 => 4,
                5 => 5,
                _ => 6,
            },
            fixed_tries: if level <= 2 { 5 } else { 2 },
            max_lpc: match level {
                0..=2 => 0,
                3 => 6,
                4..=6 => 8,
                _ => 12,
            },
            lpc_tries: match level {
                3 | 4 => 1,
                5 => 2,
                6 => 3,
                _ => 4,
            },
            // As the reference encoder: more precision for longer blocks.
            precision: 12,
        }
    }
}

/// The chosen coding of one channel.
#[derive(Clone, Debug)]
pub(super) struct Plan {
    pub(super) kind: Kind,
    /// Wasted low bits shifted out of every sample.
    pub(super) wasted: u32,
    /// Bits per sample after the wasted bits are gone.
    pub(super) bps: u32,
    /// Exact size of the subframe in bits, header included.
    pub(super) bits: u64,
    /// Folded residuals of a predicted subframe.
    pub(super) res: Vec<u32>,
    pub(super) rice: RicePlan,
    pub(super) precision: u32,
    pub(super) shift: u32,
    /// Quantised coefficients, the one for the most recent sample first.
    pub(super) coefs: [i32; MAX_ORDER],
}

impl Plan {
    pub(super) fn new(block: usize) -> Self {
        Self {
            kind: Kind::Verbatim,
            wasted: 0,
            bps: 0,
            bits: 0,
            res: Vec::with_capacity(block),
            rice: RicePlan::new(),
            precision: 0,
            shift: 0,
            coefs: [0; MAX_ORDER],
        }
    }
}

pub(super) fn is_constant<T: Sample>(x: &[T]) -> bool {
    let first: i64 = x[0].into();
    x.iter().all(|&v| v.into() == first)
}

pub(super) struct Analyzer {
    level: Level,
    /// Candidate residual and Rice plan; swapped into the `Plan` when they win.
    cand: Vec<u32>,
    cand_rice: RicePlan,
    rice: RiceSearch,
    /// Samples with the wasted bits shifted out.
    pool32: Vec<i32>,
    pool64: Vec<i64>,
    window: Vec<f64>,
    windowed: Vec<f64>,
    lp: Box<[[f64; MAX_ORDER]; MAX_ORDER]>,
    acc: lpc::Accumulators,
}

impl Analyzer {
    pub(super) fn new(level: Level) -> Self {
        Self {
            level,
            cand: Vec::with_capacity(level.block),
            cand_rice: RicePlan::new(),
            rice: RiceSearch::new(),
            pool32: Vec::with_capacity(level.block),
            pool64: Vec::with_capacity(level.block),
            window: Vec::with_capacity(level.block),
            windowed: Vec::with_capacity(level.block),
            lp: Box::new([[0.0; MAX_ORDER]; MAX_ORDER]),
            acc: lpc::Accumulators::default(),
        }
    }

    /// Constant or verbatim, nothing cleverer: the baseline every coding must beat.
    pub(super) fn trivial<T: Sample>(&mut self, x: &[T], bps: u32, plan: &mut Plan) {
        plan.wasted = 0;
        plan.bps = bps;
        if is_constant(x) {
            plan.kind = Kind::Constant;
            plan.bits = 8 + u64::from(bps);
        } else {
            plan.kind = Kind::Verbatim;
            plan.bits = 8 + x.len() as u64 * u64::from(bps);
        }
    }

    /// Picks the cheapest coding of `x`, `bps` bits per sample, into `plan`.
    pub(super) fn analyze<T: Sample>(&mut self, x: &[T], bps: u32, plan: &mut Plan) {
        if is_constant(x) {
            self.trivial(x, bps, plan);
            return;
        }
        let any = x.iter().fold(0i64, |acc, &v| acc | v.into());
        // Not constant, so some sample is non-zero and the shift is below the depth.
        let wasted = any.trailing_zeros();
        if wasted == 0 {
            self.search(x, bps, 0, plan);
        } else {
            let mut shifted = std::mem::take(T::pool(self));
            shifted.clear();
            shifted.extend(x.iter().map(|&v| v.shr(wasted)));
            self.search(&shifted, bps - wasted, wasted, plan);
            *T::pool(self) = shifted;
        }
    }

    /// `x` has its wasted bits removed already; `bps` is the depth that is left.
    fn search<T: Sample>(&mut self, x: &[T], bps: u32, wasted: u32, plan: &mut Plan) {
        let n = x.len();
        let head = 8 + u64::from(wasted);
        plan.wasted = wasted;
        plan.bps = bps;
        plan.kind = Kind::Verbatim;
        plan.bits = head + n as u64 * u64::from(bps);
        let top = 4usize.min(n - 1);
        // Rank the orders by a one-pass estimate; the leaders are then coded exactly.
        let mut rank = [0usize, 1, 2, 3, 4];
        if self.level.fixed_tries <= top && n >= 5 {
            let sums = fixed::abs_sums(x);
            let count = (n - 4) as u64;
            let est =
                |o: usize| fixed::rough_bits(sums[o], count) + o as u64 * u64::from(bps) * 256;
            rank[..=top].sort_by_key(|&o| est(o));
        }
        for &order in &rank[..self.level.fixed_tries.min(top + 1)] {
            if !fixed::residual(x, order, bps, &mut self.cand) {
                continue;
            }
            self.rice.plan(
                &self.cand,
                n,
                order,
                self.level.rice_order,
                &mut self.cand_rice,
            );
            let bits = head + order as u64 * u64::from(bps) + self.cand_rice.bits;
            if bits < plan.bits {
                plan.bits = bits;
                plan.kind = Kind::Fixed(order as u8);
                std::mem::swap(&mut plan.res, &mut self.cand);
                std::mem::swap(&mut plan.rice, &mut self.cand_rice);
            }
        }
        if self.level.max_lpc > 0 && n >= 16 {
            self.search_lpc(x, bps, head, plan);
        }
    }

    /// Linear prediction: one Levinson run gives every order; the orders that look best by
    /// estimate are then coded for real and compared by exact bits.
    fn search_lpc<T: Sample>(&mut self, x: &[T], bps: u32, head: u64, plan: &mut Plan) {
        let n = x.len();
        let max_order = self.level.max_lpc.min(n - 1);
        if self.window.len() != n {
            lpc::window(n, &mut self.window);
        }
        self.windowed.clear();
        self.windowed.extend(
            x.iter()
                .zip(&self.window)
                .map(|(&v, &w)| v.into() as f64 * w),
        );
        let mut autoc = [0.0f64; MAX_ORDER + 1];
        lpc::autocorrelation(&self.windowed, &mut autoc[..=max_order]);
        autoc[0] *= 1.0 + 1e-9;
        let mut err = [0.0f64; MAX_ORDER];
        let orders = lpc::levinson(&autoc, max_order, &mut self.lp, &mut err);
        if orders == 0 {
            return;
        }
        let precision = self.level.precision;
        // Estimated bits per order, as the reference encoder does: the error energy gives the
        // residual size, each coefficient costs its precision, each warm-up sample the depth.
        let scale = 0.5 / n as f64;
        let mut est = [(f64::MAX, 0usize); MAX_ORDER];
        for k in 0..orders {
            let e = err[k] * scale;
            let per = if e > 0.0 {
                (0.5 * lpc::log2(e)).max(0.0)
            } else {
                0.0
            };
            let order = k + 1;
            est[k] = (
                per * (n - order) as f64 + (order as u64 * u64::from(precision + bps)) as f64,
                order,
            );
        }
        let tries = self.level.lpc_tries.min(orders);
        for t in 0..tries {
            // Selection of the next best estimate; ties go to the lower order.
            let mut at = t;
            for i in t + 1..orders {
                if est[i].0 < est[at].0 {
                    at = i;
                }
            }
            est.swap(t, at);
            let order = est[t].1;
            let mut q = [0i32; MAX_ORDER];
            let Some(shift) =
                lpc::quantize(&self.lp[order - 1][..order], precision, &mut q[..order])
            else {
                continue;
            };
            if !lpc::residual(
                x,
                &lpc::Quantized {
                    coefs: &q[..order],
                    shift,
                    precision,
                },
                bps,
                &mut self.cand,
                &mut self.acc,
            ) {
                continue;
            }
            self.rice.plan(
                &self.cand,
                n,
                order,
                self.level.rice_order,
                &mut self.cand_rice,
            );
            let bits = head
                + order as u64 * u64::from(bps)
                + 9
                + order as u64 * u64::from(precision)
                + self.cand_rice.bits;
            if bits < plan.bits {
                plan.bits = bits;
                plan.kind = Kind::Lpc(order as u8);
                plan.precision = precision;
                plan.shift = shift;
                plan.coefs = q;
                std::mem::swap(&mut plan.res, &mut self.cand);
                std::mem::swap(&mut plan.rice, &mut self.cand_rice);
            }
        }
    }
}
