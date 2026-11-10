//! Chooses how to code one channel of one block: the cheapest subframe, by exact bit count.

/// A sample type the analysis runs on: `i32` for every channel but the side channel of a
/// 32-bit stream, which needs `i64`.
pub(super) trait Sample: Copy + Into<i64> {}
impl Sample for i32 {}
impl Sample for i64 {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Constant,
    Verbatim,
}

/// The chosen coding of one channel.
#[derive(Clone, Debug)]
pub(super) struct Plan {
    pub(super) kind: Kind,
    /// Bits per sample before the wasted bits are shifted out (the subframe's full depth).
    pub(super) bps: u32,
    /// Exact size of the subframe in bits, header included.
    pub(super) bits: u64,
}

impl Plan {
    pub(super) fn new() -> Self {
        Self {
            kind: Kind::Verbatim,
            bps: 0,
            bits: 0,
        }
    }
}

pub(super) struct Analyzer {}

impl Analyzer {
    pub(super) fn new(_block: usize) -> Self {
        Self {}
    }

    pub(super) fn analyze<T: Sample>(&mut self, x: &[T], bps: u32, plan: &mut Plan) {
        let first: i64 = x[0].into();
        plan.bps = bps;
        if x.iter().all(|&v| v.into() == first) {
            plan.kind = Kind::Constant;
            plan.bits = 8 + u64::from(bps);
        } else {
            plan.kind = Kind::Verbatim;
            plan.bits = 8 + x.len() as u64 * u64::from(bps);
        }
    }
}
