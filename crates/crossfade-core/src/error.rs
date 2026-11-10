/// Everything `crossfade-core` can fail with.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("denominator is zero")]
    ZeroDenominator,
    #[error("division by zero")]
    DivisionByZero,
    #[error("arithmetic overflow")]
    Overflow,
    #[error("time base must be strictly positive")]
    InvalidTimeBase,
}
