//! Chooses how to code one channel of one block: the cheapest subframe, by exact bit count.

use super::fixed;
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
        for order in 0..=4usize.min(n - 1) {
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
    }
}
