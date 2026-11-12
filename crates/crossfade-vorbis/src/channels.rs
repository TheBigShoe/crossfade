//! Where Vorbis puts its channels (specification section 4.3.9).

/// A speaker position. Vorbis says "rear" where WAVE says "back".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Speaker {
    FrontLeft,
    FrontCenter,
    FrontRight,
    RearLeft,
    RearRight,
    SideLeft,
    SideRight,
    RearCenter,
    Lfe,
}

/// The speaker of each channel, in Vorbis's channel order, for the counts the specification
/// defines (1 to 8); `None` for any other count, whose order is the application's.
pub fn speakers(channels: u8) -> Option<&'static [Speaker]> {
    use Speaker::*;
    Some(match channels {
        1 => &[FrontCenter],
        2 => &[FrontLeft, FrontRight],
        3 => &[FrontLeft, FrontCenter, FrontRight],
        4 => &[FrontLeft, FrontRight, RearLeft, RearRight],
        5 => &[FrontLeft, FrontCenter, FrontRight, RearLeft, RearRight],
        6 => &[FrontLeft, FrontCenter, FrontRight, RearLeft, RearRight, Lfe],
        7 => &[
            FrontLeft,
            FrontCenter,
            FrontRight,
            SideLeft,
            SideRight,
            RearCenter,
            Lfe,
        ],
        8 => &[
            FrontLeft,
            FrontCenter,
            FrontRight,
            SideLeft,
            SideRight,
            RearLeft,
            RearRight,
            Lfe,
        ],
        _ => return None,
    })
}

#[cfg(test)]
#[path = "channels_tests.rs"]
mod tests;
