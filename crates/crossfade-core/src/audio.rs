use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SampleFormat {
    U8,
    S16,
    S24,
    S32,
    F32,
    F64,
}

impl SampleFormat {
    pub fn bytes_per_sample(self) -> usize {
        match self {
            Self::U8 => 1,
            Self::S16 => 2,
            Self::S24 => 3,
            Self::S32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }

    pub fn bits(self) -> usize {
        self.bytes_per_sample() * 8
    }

    pub fn is_float(self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    FrontLeft,
    FrontRight,
    FrontCenter,
    Lfe,
    BackLeft,
    BackRight,
    SideLeft,
    SideRight,
}

/// Either an ordered list of distinct speaker positions, or just a count
/// when the source does not say where the channels go.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChannelLayout {
    positions: Option<Vec<Channel>>,
    count: u16,
}

impl ChannelLayout {
    pub fn mono() -> Self {
        Self {
            positions: Some(vec![Channel::FrontCenter]),
            count: 1,
        }
    }

    pub fn stereo() -> Self {
        Self {
            positions: Some(vec![Channel::FrontLeft, Channel::FrontRight]),
            count: 2,
        }
    }

    pub fn from_positions(positions: Vec<Channel>) -> Result<Self, Error> {
        if positions.is_empty() {
            return Err(Error::EmptyChannelLayout);
        }
        for (i, c) in positions.iter().enumerate() {
            if positions[..i].contains(c) {
                return Err(Error::DuplicateChannel(*c));
            }
        }
        // Distinct positions of a small enum: the count always fits.
        let count = u16::try_from(positions.len()).map_err(|_| Error::Overflow)?;
        Ok(Self {
            positions: Some(positions),
            count,
        })
    }

    pub fn unpositioned(count: u16) -> Result<Self, Error> {
        if count == 0 {
            return Err(Error::EmptyChannelLayout);
        }
        Ok(Self {
            positions: None,
            count,
        })
    }

    pub fn channels(&self) -> u16 {
        self.count
    }

    pub fn positions(&self) -> Option<&[Channel]> {
        self.positions.as_deref()
    }
}

/// Fields are private so a spec can only exist in a valid state.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AudioSpec {
    sample_rate: u32,
    format: SampleFormat,
    layout: ChannelLayout,
}

impl AudioSpec {
    pub fn new(
        sample_rate: u32,
        format: SampleFormat,
        layout: ChannelLayout,
    ) -> Result<Self, Error> {
        if sample_rate == 0 {
            return Err(Error::ZeroSampleRate);
        }
        Ok(Self {
            sample_rate,
            format,
            layout,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn format(&self) -> SampleFormat {
        self.format
    }

    pub fn layout(&self) -> &ChannelLayout {
        &self.layout
    }

    /// At most `u16::MAX * 8`, so the product cannot overflow `usize`.
    pub fn bytes_per_frame(&self) -> usize {
        usize::from(self.layout.channels()) * self.format.bytes_per_sample()
    }
}

#[cfg(test)]
mod tests {
    use crate::{AudioSpec, Channel, ChannelLayout, Error, SampleFormat};

    #[test]
    fn sample_format_sizes() {
        let cases = [
            (SampleFormat::U8, 1, 8, false),
            (SampleFormat::S16, 2, 16, false),
            (SampleFormat::S24, 3, 24, false),
            (SampleFormat::S32, 4, 32, false),
            (SampleFormat::F32, 4, 32, true),
            (SampleFormat::F64, 8, 64, true),
        ];
        for (f, bytes, bits, float) in cases {
            assert_eq!(f.bytes_per_sample(), bytes, "{f:?}");
            assert_eq!(f.bits(), bits, "{f:?}");
            assert_eq!(f.is_float(), float, "{f:?}");
        }
    }

    #[test]
    fn mono_and_stereo() {
        assert_eq!(ChannelLayout::mono().channels(), 1);
        assert_eq!(
            ChannelLayout::mono().positions(),
            Some(&[Channel::FrontCenter][..])
        );
        assert_eq!(ChannelLayout::stereo().channels(), 2);
        assert_eq!(
            ChannelLayout::stereo().positions(),
            Some(&[Channel::FrontLeft, Channel::FrontRight][..])
        );
    }

    #[test]
    fn layout_from_positions_keeps_order() {
        let l = ChannelLayout::from_positions(vec![
            Channel::Lfe,
            Channel::FrontLeft,
            Channel::SideRight,
        ])
        .unwrap();
        assert_eq!(l.channels(), 3);
        assert_eq!(
            l.positions(),
            Some(&[Channel::Lfe, Channel::FrontLeft, Channel::SideRight][..])
        );
    }

    #[test]
    fn layout_rejects_empty_and_duplicates() {
        assert_eq!(
            ChannelLayout::from_positions(vec![]),
            Err(Error::EmptyChannelLayout)
        );
        assert_eq!(
            ChannelLayout::from_positions(vec![
                Channel::FrontLeft,
                Channel::FrontRight,
                Channel::FrontLeft
            ]),
            Err(Error::DuplicateChannel(Channel::FrontLeft))
        );
    }

    #[test]
    fn unpositioned_layout_counts_only() {
        let l = ChannelLayout::unpositioned(6).unwrap();
        assert_eq!(l.channels(), 6);
        assert_eq!(l.positions(), None);
        assert_eq!(
            ChannelLayout::unpositioned(0),
            Err(Error::EmptyChannelLayout)
        );
        assert_ne!(
            ChannelLayout::unpositioned(2).unwrap(),
            ChannelLayout::stereo()
        );
    }

    #[test]
    fn audio_spec_validates_rate_and_sizes_frames() {
        let spec = AudioSpec::new(44100, SampleFormat::S24, ChannelLayout::stereo()).unwrap();
        assert_eq!(spec.sample_rate(), 44100);
        assert_eq!(spec.format(), SampleFormat::S24);
        assert_eq!(spec.layout(), &ChannelLayout::stereo());
        assert_eq!(spec.bytes_per_frame(), 6);
        assert_eq!(
            AudioSpec::new(0, SampleFormat::S16, ChannelLayout::mono()),
            Err(Error::ZeroSampleRate)
        );
    }

    #[test]
    fn bytes_per_frame_cannot_overflow() {
        let spec = AudioSpec::new(
            1,
            SampleFormat::F64,
            ChannelLayout::unpositioned(u16::MAX).unwrap(),
        )
        .unwrap();
        assert_eq!(spec.bytes_per_frame(), 8 * usize::from(u16::MAX));
    }
}
