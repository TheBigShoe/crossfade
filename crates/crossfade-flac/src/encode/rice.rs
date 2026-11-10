//! Partitioned Rice coding of a residual (RFC 9639 section 9.2.7): the search for the
//! cheapest partitioning and parameters, by exact bit count, and the writer.

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

pub(super) struct RiceSearch {}

impl RiceSearch {
    pub(super) fn new() -> Self {
        Self {}
    }

    /// `u` holds the zigzag-folded residuals of a block of `block` samples predicted at
    /// `pred_order`; partition orders up to `max_order` are tried.
    pub(super) fn plan(
        &mut self,
        u: &[u32],
        block: usize,
        pred_order: usize,
        max_order: u32,
        out: &mut RicePlan,
    ) {
        let _ = (u, block, pred_order, max_order, out);
        unimplemented!()
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
                if block % parts != 0 || (block >> po) <= pred {
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
