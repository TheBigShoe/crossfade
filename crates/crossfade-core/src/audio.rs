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
