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
    /// The samples as `i32`, when that is what they are: the fast kernels take only these.
    fn as_i32(x: &[Self]) -> Option<&[i32]>;
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
    fn as_i32(x: &[Self]) -> Option<&[i32]> {
        Some(x)
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
    fn as_i32(_: &[Self]) -> Option<&[i32]> {
        None
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

/// The ridge every fit gets: a hair of white noise keeps the recursion finite on a signal
/// that is nearly perfectly predictable.
const BASE_RIDGE: f64 = 1e-15;
/// Bits per sample above its estimate that makes a fit suspect.
const GAP_BITS: f64 = 3.5;
const REFINE_PASSES: usize = 8;

/// One analysis window for LPC: its span in sixtieths of the block, and how many orders get an
/// exact search from it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Apodization {
    from: usize,
    to: usize,
    tries: usize,
    /// The taper at each end is this fraction (1 / `taper`) of the window's span.
    taper: usize,
}

const fn apod(from: usize, to: usize, tries: usize) -> Apodization {
    Apodization {
        from,
        to,
        tries,
        taper: 4,
    }
}

static WINDOWS: [&[Apodization]; 9] = [
    &[],
    &[],
    &[],
    &[apod(0, 60, 1)],
    &[apod(0, 60, 1)],
    &[apod(0, 60, 2)],
    &[apod(0, 60, 3)],
    &[apod(0, 60, 4)],
    &[apod(0, 60, 3), apod(0, 40, 1), apod(20, 60, 1)],
];

/// What a level asks of the search.
#[derive(Clone, Copy, Debug)]
pub(super) struct Level {
    pub(super) block: usize,
    /// Extra ridge amounts (relative, added to the autocorrelation at lag 0) tried when the
    /// first fit lands far above its estimate.
    pub(super) ridges: &'static [f64],
    pub(super) stereo: Stereo,
    /// Largest Rice partition order tried.
    pub(super) rice_order: u32,
    /// How many fixed orders, best by estimate first, get an exact search.
    pub(super) fixed_tries: usize,
    /// Largest LPC order; 0 means fixed predictors only.
    pub(super) max_lpc: usize,
    /// The analysis windows, each with its own Levinson run; partial windows follow
    /// non-stationary audio better.
    pub(super) windows: &'static [Apodization],
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
            windows: WINDOWS[usize::from(level.min(8))],
            ridges: match level {
                0..=4 => &[],
                5 | 6 => &[1e-11, 1e-8],
                7 => &[1e-13, 1e-10, 1e-7],
                _ => &[1e-14, 1e-12, 1e-10, 1e-8, 1e-6],
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

/// Quantised LPC coefficient precision in bits (sign included) for a block of `block` samples
/// of a `depth`-bit stream, as the reference encoder chooses: more bits for longer blocks and
/// deeper samples, where the rounding error of a coefficient times the sample size would
/// otherwise dominate the residual of a predictable signal.
pub(super) fn precision_for(block: usize, depth: u32) -> u32 {
    let p = if depth < 16 {
        (2 + depth / 2).max(5)
    } else if depth == 16 {
        match block {
            0..=192 => 7,
            193..=384 => 8,
            385..=576 => 9,
            577..=1152 => 10,
            1153..=2304 => 11,
            2305..=4608 => 12,
            _ => 13,
        }
    } else {
        match block {
            0..=384 => 12,
            385..=1152 => 13,
            _ => 15,
        }
    };
    p.min(15)
}

pub(super) struct Analyzer {
    /// The stream's bit depth (not a side channel's or a wasted-bits-reduced one).
    depth: u32,
    level: Level,
    /// Candidate residual and Rice plan; swapped into the `Plan` when they win.
    cand: Vec<u32>,
    cand_rice: RicePlan,
    rice: RiceSearch,
    /// Samples with the wasted bits shifted out.
    pool32: Vec<i32>,
    pool64: Vec<i64>,
    /// One window per apodization of the level, and the block length they were made for.
    windows: Vec<Vec<f64>>,
    window_len: usize,
    /// The unridged autocorrelation from each window, for the current block.
    autocs: Vec<[f64; MAX_ORDER + 1]>,
    reg: f64,
    windowed: Vec<f64>,
    windowed_int: Vec<i32>,
    /// Largest magnitude in the block being searched.
    peak: u64,
    lp: Box<[[f64; MAX_ORDER]; MAX_ORDER]>,
    acc: lpc::Accumulators,
}

impl Analyzer {
    /// `wide`: samples may need `i64` (a 32-bit stereo stream's side channel).
    pub(super) fn new(level: Level, depth: u32, wide: bool) -> Self {
        let wide_cap = if wide { level.block } else { 0 };
        Self {
            depth,
            level,
            cand: Vec::with_capacity(level.block),
            cand_rice: RicePlan::new(),
            rice: RiceSearch::new(),
            pool32: Vec::with_capacity(level.block),
            pool64: Vec::with_capacity(wide_cap),
            windows: level
                .windows
                .iter()
                .map(|_| Vec::with_capacity(level.block))
                .collect(),
            window_len: 0,
            autocs: vec![[0.0; MAX_ORDER + 1]; level.windows.len()],
            reg: 1e-15,
            windowed: Vec::with_capacity(level.block),
            windowed_int: Vec::with_capacity(level.block),
            peak: 0,
            lp: Box::new([[0.0; MAX_ORDER]; MAX_ORDER]),
            acc: lpc::Accumulators::with_capacity(level.block, wide_cap),
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
            self.peak = x.iter().fold(0u64, |m, &v| m.max(v.into().unsigned_abs()));
            self.reg = BASE_RIDGE;
            let mut gap = 0.0f64;
            for w in 0..self.level.windows.len() {
                gap = gap.max(self.search_lpc(x, bps, head, w, plan));
            }
            // A predictable signal can sit far above the size its own error energy promises:
            // the quantised coefficients of a badly conditioned fit are too coarse. A little
            // ridge on the autocorrelation shrinks them, and each amount gives a different
            // set to cost exactly. Ordinary audio stays within a fraction of a bit of its
            // estimate and never pays for this.
            if gap > GAP_BITS {
                for &ridge in self.level.ridges {
                    self.reg = ridge;
                    // The first window's autocorrelation is still in `autocs[0]`.
                    self.fit_lpc(x, bps, head, 0, plan);
                }
            }
        }
    }

    /// Codes `x` with the quantised predictor `coefs`, keeps it in `plan` if it is the cheapest
    /// so far, and returns its size in bits (`None` when a residual does not fit 32 bits).
    #[allow(clippy::too_many_arguments)]
    fn try_coefs<T: Sample>(
        &mut self,
        x: &[T],
        coefs: &[i32],
        shift: u32,
        precision: u32,
        bps: u32,
        head: u64,
        plan: &mut Plan,
    ) -> Option<u64> {
        let (n, order) = (x.len(), coefs.len());
        if !lpc::residual(
            x,
            &lpc::Quantized {
                coefs,
                shift,
                precision,
            },
            bps,
            &mut self.cand,
            &mut self.acc,
        ) {
            return None;
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
            plan.coefs = [0; MAX_ORDER];
            plan.coefs[..order].copy_from_slice(coefs);
            std::mem::swap(&mut plan.res, &mut self.cand);
            std::mem::swap(&mut plan.rice, &mut self.cand_rice);
        }
        Some(bits)
    }

    /// Linear prediction: one Levinson run gives every order; the orders that look best by
    /// estimate are then coded for real and compared by exact bits. Returns how far, per
    /// sample, the best of them came out above its estimate.
    fn search_lpc<T: Sample>(
        &mut self,
        x: &[T],
        bps: u32,
        head: u64,
        w: usize,
        plan: &mut Plan,
    ) -> f64 {
        let n = x.len();
        let max_order = self.level.max_lpc.min(n - 1);
        if self.window_len != n {
            for (apod, window) in self.level.windows.iter().zip(&mut self.windows) {
                let span = (n * apod.from / 60, n * apod.to / 60);
                lpc::window(n, span.0, span.1, apod.taper, window);
            }
            self.window_len = n;
        }
        let apod = self.level.windows[w];
        // Only the span inside the window carries anything.
        let (from, to) = ((n * apod.from / 60).min(n), (n * apod.to / 60).min(n));
        lpc::windowed_autocorrelation(
            &x[from..to],
            &self.windows[w][from..to],
            self.peak,
            &mut lpc::Windowed {
                float: &mut self.windowed,
                int: &mut self.windowed_int,
            },
            &mut self.autocs[w][..=max_order],
        );
        self.fit_lpc(x, bps, head, w, plan)
    }

    /// The rest of [`search_lpc`](Self::search_lpc), from the autocorrelation of window `w`
    /// with the current ridge.
    fn fit_lpc<T: Sample>(
        &mut self,
        x: &[T],
        bps: u32,
        head: u64,
        w: usize,
        plan: &mut Plan,
    ) -> f64 {
        let n = x.len();
        let max_order = self.level.max_lpc.min(n - 1);
        let apod = self.level.windows[w];
        let mut autoc = self.autocs[w];
        autoc[max_order + 1..].fill(0.0);
        autoc[0] *= 1.0 + self.reg;
        let mut err = [0.0f64; MAX_ORDER];
        let orders = lpc::levinson(&autoc, max_order, &mut self.lp, &mut err);
        if orders == 0 {
            return 0.0;
        }
        let precision = precision_for(self.level.block, self.depth);
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
        let tries = apod.tries.min(orders);
        let mut gap = f64::MAX;
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
            let Some(bits) = self.try_coefs(x, &q[..order], shift, precision, bps, head, plan)
            else {
                continue;
            };
            let miss = (bits - head) as f64 / n as f64 - est[t].0 / n as f64;
            gap = gap.min(miss);
            if miss > GAP_BITS {
                // Far above the estimate: rounding each coefficient on its own is not the best
                // the quantiser can do, so look for a better integer point near it.
                lpc::refine(
                    &self.lp[order - 1][..order],
                    &mut q[..order],
                    shift,
                    precision,
                    &autoc,
                    REFINE_PASSES,
                );
                self.try_coefs(x, &q[..order], shift, precision, bps, head, plan);
            }
        }
        if gap == f64::MAX { 0.0 } else { gap }
    }
}

#[cfg(test)]
impl Analyzer {
    /// Capacities of every scratch buffer, to check that steady state does not grow them.
    pub(super) fn capacities(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.windows.iter().map(Vec::capacity).collect();
        v.extend([
            self.cand.capacity(),
            self.pool32.capacity(),
            self.pool64.capacity(),
            self.windowed.capacity(),
            self.windowed_int.capacity(),
        ]);
        v.extend(self.acc.capacities());
        v
    }
}
