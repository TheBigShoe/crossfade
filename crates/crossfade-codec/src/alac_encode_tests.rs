use super::*;
use crossfade_core::{ChannelLayout, SampleFormat::*};

fn alac_layout(channels: u8) -> ChannelLayout {
    let mask = crossfade_alac::wave_layout(channels).unwrap().mask;
    ChannelLayout::from_mask(mask, u16::from(channels)).unwrap()
}

fn spec(format: SampleFormat, channels: u8) -> AudioSpec {
    AudioSpec::new(44100, format, alac_layout(channels)).unwrap()
}

fn what(r: Result<Box<dyn Encoder>, Error>) -> String {
    match r {
        Err(Error::Unsupported { what }) => what,
        Err(e) => panic!("wrong error {e:?}"),
        Ok(_) => panic!("expected Unsupported"),
    }
}

#[test]
fn the_coded_depth_is_the_smallest_alac_depth_that_holds_the_true_one() {
    assert_eq!(coded_depth(S16, None), Some(16));
    assert_eq!(coded_depth(S16, Some(12)), Some(16));
    assert_eq!(coded_depth(S24, None), Some(24));
    assert_eq!(coded_depth(S24, Some(24)), Some(24));
    assert_eq!(coded_depth(S24, Some(20)), Some(20));
    assert_eq!(coded_depth(S24, Some(18)), Some(20));
    assert_eq!(coded_depth(S24, Some(16)), Some(20));
    assert_eq!(coded_depth(S24, Some(21)), Some(24));
    assert_eq!(coded_depth(S32, None), Some(32));
    assert_eq!(coded_depth(S32, Some(32)), Some(32));
    assert_eq!(coded_depth(U8, None), None);
    assert_eq!(coded_depth(F32, None), None);
    assert_eq!(coded_depth(F64, None), None);
}

#[test]
fn the_formats_alac_cannot_hold_are_refused_with_the_choices() {
    for f in [U8, F32, F64] {
        let w = what(encoder(&spec(f, 2), &EncodeOptions::default()));
        assert!(w.contains("--sample-format s16, s24 or s32"), "{f:?}: {w}");
    }
    for f in [S16, S24, S32] {
        assert!(encoder(&spec(f, 2), &EncodeOptions::default()).is_ok(), "{f:?}");
    }
}

#[test]
fn only_alacs_own_layout_for_the_channel_count_is_accepted() {
    for n in 1..=8 {
        assert!(encoder(&spec(S16, n), &EncodeOptions::default()).is_ok(), "{n}");
    }
    let skewed = AudioSpec::new(44100, S16, ChannelLayout::unpositioned(2).unwrap()).unwrap();
    let w = what(encoder(&skewed, &EncodeOptions::default()));
    assert!(w.contains("one layout only"), "{w}");
    let nine = AudioSpec::new(44100, S16, ChannelLayout::unpositioned(9).unwrap()).unwrap();
    assert!(what(encoder(&nine, &EncodeOptions::default())).contains("8 channels"));
}

#[test]
fn the_cookie_is_there_from_the_start_and_names_the_coded_depth() {
    let opts = EncodeOptions {
        bits_per_sample: Some(20),
        ..EncodeOptions::default()
    };
    let e = encoder(&spec(S24, 2), &opts).unwrap();
    let c = crossfade_alac::Config::parse(e.extradata()).unwrap();
    assert_eq!((c.bit_depth, c.channels, c.sample_rate), (20, 2, 44100));
    assert_eq!(c.frame_length, 4096);
}
