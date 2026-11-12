//! Four floats operated on lane by lane, written as plain arrays so the compiler vectorises
//! them without `unsafe` or a SIMD crate. Every operation is the scalar one per lane (no fused
//! multiply-add, no reassociation), so results are the same bits as scalar code.

use core::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct F4(pub(crate) [f32; 4]);

impl F4 {
    #[inline(always)]
    pub(crate) fn splat(x: f32) -> Self {
        Self([x; 4])
    }

    #[inline(always)]
    pub(crate) fn load(s: &[f32]) -> Self {
        Self([s[0], s[1], s[2], s[3]])
    }

    #[inline(always)]
    pub(crate) fn store(self, s: &mut [f32]) {
        s[..4].copy_from_slice(&self.0);
    }

    #[inline(always)]
    pub(crate) fn rev(self) -> Self {
        let [a, b, c, d] = self.0;
        Self([d, c, b, a])
    }
}

macro_rules! lanewise {
    ($tr:ident, $f:ident, $op:tt) => {
        impl $tr for F4 {
            type Output = F4;
            #[inline(always)]
            fn $f(self, o: F4) -> F4 {
                let (a, b) = (self.0, o.0);
                F4([a[0] $op b[0], a[1] $op b[1], a[2] $op b[2], a[3] $op b[3]])
            }
        }
    };
}
lanewise!(Add, add, +);
lanewise!(Sub, sub, -);
lanewise!(Mul, mul, *);

impl Neg for F4 {
    type Output = F4;
    #[inline(always)]
    fn neg(self) -> F4 {
        let a = self.0;
        F4([-a[0], -a[1], -a[2], -a[3]])
    }
}

impl AddAssign for F4 {
    #[inline(always)]
    fn add_assign(&mut self, o: F4) {
        *self = *self + o;
    }
}

impl SubAssign for F4 {
    #[inline(always)]
    fn sub_assign(&mut self, o: F4) {
        *self = *self - o;
    }
}

/// What the filterbank kernels need of one float or four: written once, instantiated for a
/// four-lane body and a scalar tail.
pub(crate) trait Lane:
    Copy + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Neg<Output = Self>
{
    fn splat(x: f32) -> Self;
    /// `s[0], s[stride], s[2 * stride], ...`
    fn gather(s: &[f32], stride: usize) -> Self;
    fn scatter(self, s: &mut [f32], stride: usize);
}

impl Lane for f32 {
    #[inline(always)]
    fn splat(x: f32) -> Self {
        x
    }
    #[inline(always)]
    fn gather(s: &[f32], _: usize) -> Self {
        s[0]
    }
    #[inline(always)]
    fn scatter(self, s: &mut [f32], _: usize) {
        s[0] = self;
    }
}

impl Lane for F4 {
    #[inline(always)]
    fn splat(x: f32) -> Self {
        F4::splat(x)
    }
    #[inline(always)]
    fn gather(s: &[f32], stride: usize) -> Self {
        F4([s[0], s[stride], s[2 * stride], s[3 * stride]])
    }
    #[inline(always)]
    fn scatter(self, s: &mut [f32], stride: usize) {
        s[0] = self.0[0];
        s[stride] = self.0[1];
        s[2 * stride] = self.0[2];
        s[3 * stride] = self.0[3];
    }
}
