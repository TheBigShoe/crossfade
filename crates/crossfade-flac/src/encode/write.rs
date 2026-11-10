//! Writes a chosen subframe.

use super::analyze::{Kind, Plan, Sample};
use super::bitsink::BitSink;

pub(super) fn write_subframe<T: Sample>(sink: &mut BitSink, plan: &Plan, x: &[T]) {
    sink.put(0, 1);
    match plan.kind {
        Kind::Constant => {
            sink.put(0, 7);
            sink.put_signed(x[0].into(), plan.bps);
        }
        Kind::Verbatim => {
            sink.put(1 << 1, 7);
            for &v in x {
                sink.put_signed(v.into(), plan.bps);
            }
        }
    }
}
