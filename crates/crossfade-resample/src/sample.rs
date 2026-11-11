//! The sample type the kernels are generic over. Only `f64` is implemented today; an `f32`
//! path is an `impl Sample for f32` plus its tests, since tables and buffers are all `Vec<S>`.

use std::fmt::Debug;
use std::ops::{Add, Div, Mul, Sub};

mod private {
    pub trait Sealed {}
    impl Sealed for f64 {}
}

/// A floating-point sample the resampler can work in. Sealed: the kernels rely on IEEE 754
/// semantics that only the primitive float types give.
pub trait Sample:
    Copy
    + Default
    + PartialEq
    + Debug
    + Send
    + Sync
    + 'static
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + private::Sealed
{
    const ZERO: Self;
    const ONE: Self;
    /// Rounds a designed coefficient to this type.
    fn from_f64(x: f64) -> Self;
}

impl Sample for f64 {
    const ZERO: Self = 0.0;
    const ONE: Self = 1.0;
    fn from_f64(x: f64) -> Self {
        x
    }
}
