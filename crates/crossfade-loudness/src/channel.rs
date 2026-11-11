//! What each channel of the interleaved input is, which sets its weight in the measurement.

/// A channel's role. BS.1770-4 weights: left, right and centre 1.0, surrounds 1.41, LFE not
/// measured. `Unweighted` is for any other full-range channel (a mono file's only channel, a
/// height channel) and counts as 1.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ChannelRole {
    Left,
    Right,
    Centre,
    LeftSurround,
    RightSurround,
    /// Left out of loudness (EBU Tech 3341 2.10), but its peaks are measured.
    Lfe,
    Unweighted,
    /// Ignored entirely: neither loudness nor peaks.
    Excluded,
}

impl ChannelRole {
    /// The weight `G` in the BS.1770-4 sum; 0 for [`Lfe`](Self::Lfe) and
    /// [`Excluded`](Self::Excluded).
    pub fn weight(self) -> f64 {
        match self {
            Self::Left | Self::Right | Self::Centre | Self::Unweighted => 1.0,
            Self::LeftSurround | Self::RightSurround => 1.41,
            Self::Lfe | Self::Excluded => 0.0,
        }
    }

    /// The usual SMPTE/ITU order for 1 (mono, one channel counted once), 2, 3 (L R C),
    /// 4 (L R Ls Rs), 5 (L R C Ls Rs) and 6 (L R C LFE Ls Rs) channels; any other count has
    /// no standard layout and the caller names the roles.
    pub fn standard_layout(channels: usize) -> Option<&'static [ChannelRole]> {
        use ChannelRole::{Centre, Left, LeftSurround, Lfe, Right, RightSurround, Unweighted};
        Some(match channels {
            1 => &[Unweighted],
            2 => &[Left, Right],
            3 => &[Left, Right, Centre],
            4 => &[Left, Right, LeftSurround, RightSurround],
            5 => &[Left, Right, Centre, LeftSurround, RightSurround],
            6 => &[Left, Right, Centre, Lfe, LeftSurround, RightSurround],
            _ => return None,
        })
    }
}
