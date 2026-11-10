//! Partitioned Rice coding of a residual (RFC 9639 section 9.2.7): the search for the
//! cheapest partitioning and parameters, by exact bit count, and the writer.

use super::bitsink::BitSink;

pub(super) const MAX_PARTS: usize = 256;
/// Marks a partition coded as raw signed values.
pub(super) const RAW: u8 = 255;

#[derive(Clone, Debug)]
pub(super) struct RicePlan {
    /// 5-bit parameters (otherwise 4-bit).
    pub(super) method5: bool,
    pub(super) order: u32,
    /// Per partition: the Rice parameter, or `RAW`.
    pub(super) params: [u8; MAX_PARTS],
    /// Per raw partition: the bits per value.
    pub(super) widths: [u8; MAX_PARTS],
    /// The whole residual section, its 6 header bits included.
    pub(super) bits: u64,
}

impl RicePlan {
    pub(super) fn new() -> Self {
        Self {
            method5: false,
            order: 0,
            params: [0; MAX_PARTS],
            widths: [0; MAX_PARTS],
            bits: 0,
        }
    }
}

const UNKNOWN: u64 = u64::MAX;
/// Largest parameter of each method (the next code is the escape).
const KMAX4: u32 = 14;
const KMAX5: u32 = 30;
/// Slots per partition in the cost cache.
const KS: usize = 32;

/// Scratch for the search, reused from block to block.
pub(super) struct RiceSearch {
    /// `[partition * KS + k]`: exact bits of the partition's values at parameter `k`, the
    /// parameter field excluded; `UNKNOWN` until computed. Each cost is a pass over the
    /// partition, so only the few parameters near the optimum are ever computed: the cost is
    /// convex in `k` (each `u >> k` is), which also bounds a merged partition's optimum
    /// between its parts' optima.
    cache: Vec<u64>,
    bounds: [usize; MAX_PARTS + 1],
    sum: [u64; MAX_PARTS],
    max: [u32; MAX_PARTS],
    kstar: [u8; MAX_PARTS],
    /// The partitioning being costed.
    trial_params: [u8; MAX_PARTS],
    trial_widths: [u8; MAX_PARTS],
}

/// Bits a raw value needs, 0 for zero.
fn raw_width(max_folded: u32) -> u32 {
    if max_folded == 0 {
        0
    } else {
        33 - (max_folded >> 1).leading_zeros()
    }
}

impl RiceSearch {
    pub(super) fn new() -> Self {
        Self {
            cache: vec![UNKNOWN; MAX_PARTS * KS],
            bounds: [0; MAX_PARTS + 1],
            sum: [0; MAX_PARTS],
            max: [0; MAX_PARTS],
            kstar: [0; MAX_PARTS],
            trial_params: [0; MAX_PARTS],
            trial_widths: [0; MAX_PARTS],
        }
    }

    fn cost(&mut self, u: &[u32], j: usize, k: u32) -> u64 {
        let slot = j * KS + k as usize;
        if self.cache[slot] == UNKNOWN {
            let part = &u[self.bounds[j]..self.bounds[j + 1]];
            let shifted: u64 = part.iter().map(|&v| u64::from(v >> k)).sum();
            self.cache[slot] = part.len() as u64 * u64::from(k + 1) + shifted;
        }
        self.cache[slot]
    }

    /// `u` holds the zigzag-folded residuals of a block of `block` samples predicted at
    /// `pred_order`, so it is `block - pred_order` long; partition orders up to `max_order`
    /// (at most 8) are tried.
    pub(super) fn plan(
        &mut self,
        u: &[u32],
        block: usize,
        pred_order: usize,
        max_order: u32,
        out: &mut RicePlan,
    ) {
        debug_assert_eq!(u.len() + pred_order, block);
        let mut top = max_order.min(8);
        while top > 0 && (!block.is_multiple_of(1 << top) || (block >> top) <= pred_order) {
            top -= 1;
        }
        let parts = 1usize << top;
        let size = block >> top;
        self.bounds[0] = 0;
        for j in 1..=parts {
            self.bounds[j] = if j == parts {
                u.len()
            } else {
                j * size - pred_order
            };
        }
        for j in 0..parts {
            let part = &u[self.bounds[j]..self.bounds[j + 1]];
            self.sum[j] = part.iter().map(|&v| u64::from(v)).sum();
            self.max[j] = part.iter().copied().max().unwrap_or(0);
        }
        self.cache[..parts * KS].fill(UNKNOWN);

        self.search(u, top, false, out);
        // 5-bit parameters only pay when some partition wants a parameter above 14; by
        // convexity that shows in the finest partitions.
        let wants_more = (0..parts).any(|j| {
            u32::from(self.kstar[j]) == KMAX4 && self.cost(u, j, KMAX4 + 1) < self.cost(u, j, KMAX4)
        });
        if wants_more {
            let mut other = out.clone();
            self.search(u, top, true, &mut other);
            if other.bits < out.bits {
                *out = other;
            }
        }
    }

    /// The best plan for one method, over every partition order up to `top`.
    fn search(&mut self, u: &[u32], top: u32, method5: bool, out: &mut RicePlan) {
        let parts = 1usize << top;
        let (pbits, kmax) = if method5 { (5, KMAX5) } else { (4, KMAX4) };
        for j in 0..parts {
            let len = (self.bounds[j + 1] - self.bounds[j]) as u64;
            let mean = self.sum[j].checked_div(len).unwrap_or(0);
            // The optimum is near log2 of the mean; convexity makes a downhill walk exact.
            let mut k = (64 - mean.leading_zeros()).saturating_sub(1).min(kmax);
            loop {
                if k < kmax && self.cost(u, j, k + 1) < self.cost(u, j, k) {
                    k += 1;
                } else if k > 0 && self.cost(u, j, k - 1) <= self.cost(u, j, k) {
                    k -= 1;
                } else {
                    break;
                }
            }
            self.kstar[j] = k as u8;
        }

        out.bits = u64::MAX;
        out.method5 = method5;
        for order in (0..=top).rev() {
            let per = parts >> order;
            let mut total = 6u64;
            for g in 0..1usize << order {
                let fine = g * per..(g + 1) * per;
                let lo = fine.clone().map(|j| self.kstar[j]).min().unwrap_or(0);
                let hi = fine.clone().map(|j| self.kstar[j]).max().unwrap_or(0);
                let (mut best, mut best_k) = (u64::MAX, lo);
                for k in lo..=hi {
                    let c: u64 = fine.clone().map(|j| self.cost(u, j, u32::from(k))).sum();
                    if c < best {
                        (best, best_k) = (c, k);
                    }
                }
                let count = (self.bounds[fine.end] - self.bounds[fine.start]) as u64;
                let width = raw_width(fine.clone().map(|j| self.max[j]).max().unwrap_or(0));
                let raw = 5 + count * u64::from(width);
                total += pbits;
                if width <= 31 && raw < best {
                    total += raw;
                    self.trial_params[g] = RAW;
                    self.trial_widths[g] = width as u8;
                } else {
                    total += best;
                    self.trial_params[g] = best_k;
                }
            }
            if total < out.bits {
                out.bits = total;
                out.order = order;
                let n = 1usize << order;
                out.params[..n].copy_from_slice(&self.trial_params[..n]);
                out.widths[..n].copy_from_slice(&self.trial_widths[..n]);
            }
        }
    }
}
/// Writes the residual section: method, partition order, then each partition.
pub(super) fn write(sink: &mut BitSink, plan: &RicePlan, u: &[u32], block: usize, pred: usize) {
    let (pbits, escape) = if plan.method5 { (5, 31) } else { (4, 15) };
    sink.put(u64::from(plan.method5), 2);
    sink.put(u64::from(plan.order), 4);
    let parts = 1usize << plan.order;
    let size = block >> plan.order;
    let mut at = 0;
    for p in 0..parts {
        let count = if p == 0 { size - pred } else { size };
        let part = &u[at..at + count];
        at += count;
        if plan.params[p] == RAW {
            let width = u32::from(plan.widths[p]);
            sink.put(escape, pbits);
            sink.put(u64::from(width), 5);
            for &v in part {
                let e = ((v >> 1) as i32) ^ -((v & 1) as i32);
                sink.put_signed(i64::from(e), width);
            }
        } else {
            let k = u32::from(plan.params[p]);
            sink.put(u64::from(k), pbits);
            let mask = (1u32 << k) - 1;
            for &v in part {
                let q = v >> k;
                let low = u64::from((1u32 << k) | (v & mask));
                if q + 1 + k <= 32 {
                    sink.put(low, q + 1 + k);
                } else {
                    sink.put_zeros(q);
                    sink.put(low, k + 1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold(e: i32) -> u32 {
        ((e << 1) ^ (e >> 31)) as u32
    }

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }
    }

    /// Bits of one partition at parameter `k`, counted value by value.
    fn rice_bits(part: &[u32], k: u32) -> u64 {
        part.iter()
            .map(|&u| u64::from(u >> k) + 1 + u64::from(k))
            .sum()
    }

    /// The cheapest total by trying everything: every partition order, every parameter,
    /// both methods, raw partitions.
    fn brute(u: &[u32], block: usize, pred: usize, max_order: u32) -> u64 {
        let mut best = u64::MAX;
        for method5 in [false, true] {
            let (pbits, kmax) = if method5 { (5u64, 30) } else { (4, 14) };
            for po in 0..=max_order.min(8) {
                let parts = 1usize << po;
                if !block.is_multiple_of(parts) || (block >> po) <= pred {
                    continue;
                }
                let size = block >> po;
                let mut total = 6u64;
                for p in 0..parts {
                    let lo = if p == 0 { 0 } else { p * size - pred };
                    let hi = (p + 1) * size - pred;
                    let part = &u[lo..hi];
                    let mut cost = (0..=kmax).map(|k| rice_bits(part, k)).min().unwrap();
                    let width = part
                        .iter()
                        .map(|&v| {
                            if v == 0 {
                                0
                            } else {
                                33 - (v >> 1).leading_zeros()
                            }
                        })
                        .max()
                        .unwrap_or(0);
                    if width <= 31 {
                        cost = cost.min(5 + part.len() as u64 * u64::from(width));
                    }
                    total += pbits + cost;
                }
                best = best.min(total);
            }
        }
        best
    }

    fn check(residual: &[i32], pred: usize, block: usize, max_order: u32) {
        let u: Vec<u32> = residual.iter().map(|&e| fold(e)).collect();
        let mut plan = RicePlan::new();
        RiceSearch::new().plan(&u, block, pred, max_order, &mut plan);
        assert_eq!(plan.bits, brute(&u, block, pred, max_order), "{residual:?}");
    }

    #[test]
    fn matches_a_brute_force_optimum_on_small_inputs() {
        let mut rng = Lcg(42);
        for case in 0..400 {
            let block = [16usize, 24, 32, 48, 64, 96, 17, 100][case % 8];
            let pred = case % 5;
            let spread = [0u32, 1, 3, 9, 40, 600, 70_000, 5_000_000][(case / 8) % 8];
            let loud = case % 3 == 0;
            let res: Vec<i32> = (0..block - pred)
                .map(|i| {
                    let scale = if loud && i % 16 < 4 {
                        spread.saturating_mul(64)
                    } else {
                        spread
                    };
                    let r = rng.next() as i64 % (2 * i64::from(scale) + 1) - i64::from(scale);
                    r as i32
                })
                .collect();
            check(&res, pred, block, 6);
        }
    }

    #[test]
    fn large_values_need_five_bit_parameters_or_raw_partitions() {
        let res: Vec<i32> = (0..64)
            .map(|i| if i % 2 == 0 { 1 << 29 } else { -(1 << 29) + i })
            .collect();
        check(&res, 0, 64, 4);
        let res: Vec<i32> = vec![i32::MAX, -i32::MAX, 5, -7, 1 << 20, 3, 4, 5];
        check(&res, 0, 8, 0);
    }

    #[test]
    fn all_zero_residual_is_one_raw_partition_of_zero_width() {
        let u = vec![0u32; 4096];
        let mut plan = RicePlan::new();
        RiceSearch::new().plan(&u, 4096, 0, 6, &mut plan);
        assert_eq!(plan.order, 0);
        assert_eq!(plan.params[0], RAW);
        assert_eq!(plan.widths[0], 0);
        assert_eq!(plan.bits, 6 + 4 + 5);
    }
}
