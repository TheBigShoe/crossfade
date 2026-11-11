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

    pub fn name(self) -> &'static str {
        match self {
            Self::U8 => "u8",
            Self::S16 => "s16",
            Self::S24 => "s24",
            Self::S32 => "s32",
            Self::F32 => "f32",
            Self::F64 => "f64",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "u8" => Self::U8,
            "s16" => Self::S16,
            "s24" => Self::S24,
            "s32" => Self::S32,
            "f32" => Self::F32,
            "f64" => Self::F64,
            _ => return None,
        })
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
    FrontLeftOfCenter,
    FrontRightOfCenter,
    BackCenter,
    TopCenter,
    TopFrontLeft,
    TopFrontCenter,
    TopFrontRight,
    TopBackLeft,
    TopBackCenter,
    TopBackRight,
}

/// The speaker positions of the WAVE channel mask (`WAVEFORMATEXTENSIBLE`, also FLAC's
/// `WAVEFORMATEXTENSIBLE_CHANNEL_MASK`), by bit number. Files store channels in this bit order.
const MASK_BITS: [Channel; 18] = [
    Channel::FrontLeft,
    Channel::FrontRight,
    Channel::FrontCenter,
    Channel::Lfe,
    Channel::BackLeft,
    Channel::BackRight,
    Channel::FrontLeftOfCenter,
    Channel::FrontRightOfCenter,
    Channel::BackCenter,
    Channel::SideLeft,
    Channel::SideRight,
    Channel::TopCenter,
    Channel::TopFrontLeft,
    Channel::TopFrontCenter,
    Channel::TopFrontRight,
    Channel::TopBackLeft,
    Channel::TopBackCenter,
    Channel::TopBackRight,
];

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

    /// The layout a WAVE channel mask describes, positions in bit order. `None` unless every set
    /// bit is a defined position and their count is `channels`.
    pub fn from_mask(mask: u32, channels: u16) -> Option<Self> {
        if mask >> MASK_BITS.len() != 0 || mask.count_ones() != u32::from(channels) {
            return None;
        }
        let positions = MASK_BITS
            .iter()
            .enumerate()
            .filter(|&(bit, _)| mask & (1 << bit) != 0)
            .map(|(_, &c)| c)
            .collect();
        Self::from_positions(positions).ok()
    }

    /// The WAVE channel mask, when the layout is positioned and its channels already stand in
    /// bit order (a mask is a set, so any other order would be lost).
    pub fn to_mask(&self) -> Option<u32> {
        let mut last = None;
        let mut mask = 0;
        for c in self.positions()? {
            let bit = MASK_BITS.iter().position(|m| m == c)?;
            if last.is_some_and(|l| bit < l) {
                return None;
            }
            last = Some(bit);
            mask |= 1 << bit;
        }
        Some(mask)
    }

    /// Every name [`from_name`](Self::from_name) takes, in the order help text lists them.
    pub const NAMES: [&'static str; 14] = [
        "mono",
        "stereo",
        "2.1",
        "3.0",
        "3.1",
        "4.0",
        "quad",
        "quad-side",
        "5.0",
        "5.0-back",
        "5.1",
        "5.1-back",
        "6.1",
        "7.1",
    ];

    /// A named layout. Names follow FFmpeg's, with `-back`/`-side` for its parenthesised
    /// variants so a name needs no quoting in a shell: "5.1" has side surrounds, "5.1-back"
    /// back ones.
    pub fn from_name(name: &str) -> Option<Self> {
        let mask = match name {
            "mono" => 0x4,
            "stereo" => 0x3,
            "2.1" => 0xB,
            "3.0" => 0x7,
            "3.1" => 0xF,
            "4.0" => 0x107,
            "quad" => 0x33,
            "quad-side" => 0x603,
            "5.0" => 0x607,
            "5.0-back" => 0x37,
            "5.1" => 0x60F,
            "5.1-back" => 0x3F,
            "6.1" => 0x70F,
            "7.1" => 0x63F,
            _ => return None,
        };
        Self::from_mask(mask, mask.count_ones() as u16)
    }

    /// The name of this exact layout (positions and order), if it has one.
    pub fn name(&self) -> Option<&'static str> {
        Self::NAMES
            .into_iter()
            .find(|n| Self::from_name(n).as_ref() == Some(self))
    }

    /// FLAC's default layout for `count` channels (RFC 9639 section 9.1.3, side surrounds as
    /// the reference decoder reads them); unpositioned past eight.
    pub fn default_for(count: u16) -> Result<Self, Error> {
        let name = match count {
            1 => "mono",
            2 => "stereo",
            3 => "3.0",
            4 => "quad",
            5 => "5.0",
            6 => "5.1",
            7 => "6.1",
            8 => "7.1",
            _ => return Self::unpositioned(count),
        };
        Self::from_name(name).ok_or(Error::EmptyChannelLayout)
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

    #[test]
    fn a_wave_mask_names_positions_in_bit_order() {
        use Channel::*;
        let l = ChannelLayout::from_mask(0x060F, 6).unwrap();
        assert_eq!(
            l.positions(),
            Some(&[FrontLeft, FrontRight, FrontCenter, Lfe, SideLeft, SideRight][..])
        );
        assert_eq!(l.to_mask(), Some(0x060F));
        assert_eq!(
            ChannelLayout::from_mask(0x3F, 6).unwrap().to_mask(),
            Some(0x3F)
        );
        assert_eq!(
            ChannelLayout::from_mask(0x70F, 7)
                .unwrap()
                .positions()
                .unwrap()[4],
            BackCenter
        );
    }

    #[test]
    fn every_defined_position_round_trips_through_the_mask() {
        let all = ChannelLayout::from_mask(0x3FFFF, 18).unwrap();
        assert_eq!(all.channels(), 18);
        assert_eq!(all.to_mask(), Some(0x3FFFF));
        for bit in 0..18 {
            let l = ChannelLayout::from_mask(1 << bit, 1).unwrap();
            assert_eq!(l.to_mask(), Some(1 << bit), "bit {bit}");
        }
    }

    #[test]
    fn a_mask_that_does_not_fit_the_count_or_the_table_is_refused() {
        assert_eq!(ChannelLayout::from_mask(0x3F, 5), None);
        assert_eq!(ChannelLayout::from_mask(0x3F, 7), None);
        assert_eq!(ChannelLayout::from_mask(0, 0), None);
        // Bits 18 to 30 are reserved and bit 31 means "any speaker layout".
        assert_eq!(ChannelLayout::from_mask(1 << 18, 1), None);
        assert_eq!(ChannelLayout::from_mask(0x8000_0000, 1), None);
    }

    #[test]
    fn only_a_positioned_layout_in_bit_order_has_a_mask() {
        use Channel::*;
        assert_eq!(ChannelLayout::unpositioned(2).unwrap().to_mask(), None);
        let swapped = ChannelLayout::from_positions(vec![FrontRight, FrontLeft]).unwrap();
        assert_eq!(swapped.to_mask(), None);
        assert_eq!(ChannelLayout::mono().to_mask(), Some(0x4));
        assert_eq!(ChannelLayout::stereo().to_mask(), Some(0x3));
    }
}
