//! Writes a chosen subframe.

use super::analyze::{Kind, Plan, Sample};
use super::bitsink::BitSink;
use super::rice;

// Kept out of its callers: inlined into them under fat LTO it compiled slower.
#[inline(never)]
pub(super) fn write_subframe<T: Sample>(sink: &mut BitSink, plan: &Plan, x: &[T]) {
    let kind = match plan.kind {
        Kind::Constant => 0,
        Kind::Verbatim => 1,
        Kind::Fixed(order) => 8 + u64::from(order),
        Kind::Lpc(order) => 31 + u64::from(order),
    };
    sink.put(kind, 7);
    sink.put(u64::from(plan.wasted > 0), 1);
    if plan.wasted > 0 {
        sink.put_zeros(plan.wasted - 1);
        sink.put(1, 1);
    }
    let shift = plan.wasted;
    let bps = plan.bps;
    let put = |sink: &mut BitSink, v: T| sink.put_signed(v.into() >> shift, bps);
    match plan.kind {
        Kind::Constant => put(sink, x[0]),
        Kind::Verbatim => x.iter().for_each(|&v| put(sink, v)),
        Kind::Fixed(order) => {
            let order = usize::from(order);
            x[..order].iter().for_each(|&v| put(sink, v));
            rice::write(sink, &plan.rice, &plan.res, x.len(), order);
        }
        Kind::Lpc(order) => {
            let order = usize::from(order);
            x[..order].iter().for_each(|&v| put(sink, v));
            sink.put(u64::from(plan.precision - 1), 4);
            sink.put(u64::from(plan.shift), 5);
            for &c in &plan.coefs[..order] {
                sink.put_signed(i64::from(c), plan.precision);
            }
            rice::write(sink, &plan.rice, &plan.res, x.len(), order);
        }
    }
}
