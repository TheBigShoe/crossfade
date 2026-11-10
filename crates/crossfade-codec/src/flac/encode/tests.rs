use super::*;
use crate::flac::streaminfo::StreamInfoBlock;
use crossfade_core::{AudioFrame, ChannelLayout, SampleFormat, Samples};

fn spec(rate: u32, format: SampleFormat, channels: u16) -> AudioSpec {
    AudioSpec::new(rate, format, ChannelLayout::unpositioned(channels).unwrap()).unwrap()
}

fn opts(level: Option<u8>) -> EncodeOptions {
    EncodeOptions {
        compression_level: level,
    }
}

fn unsupported(r: Result<Box<dyn Encoder>, Error>) -> String {
    match r {
        Err(Error::Unsupported { what }) => what,
        Err(e) => panic!("wrong error {e:?}"),
        Ok(_) => panic!("expected Unsupported"),
    }
}

#[test]
fn only_integer_formats_up_to_24_bits_are_accepted() {
    for f in [SampleFormat::U8, SampleFormat::S16, SampleFormat::S24] {
        assert!(encoder(&spec(44100, f, 2), &opts(None)).is_ok(), "{f:?}");
    }
    assert_eq!(
        unsupported(encoder(&spec(44100, SampleFormat::S32, 2), &opts(None))),
        "flac encoding of 32-bit samples is not supported yet; choose --sample-format s24"
    );
    for f in [SampleFormat::F32, SampleFormat::F64] {
        let what = unsupported(encoder(&spec(44100, f, 2), &opts(None)));
        assert!(what.contains("float"), "{what}");
    }
}

#[test]
fn channel_count_rate_and_level_limits_are_named() {
    assert!(encoder(&spec(44100, SampleFormat::S16, 8), &opts(None)).is_ok());
    let what = unsupported(encoder(&spec(44100, SampleFormat::S16, 9), &opts(None)));
    assert!(what.contains("8 channels"), "{what}");

    assert!(encoder(&spec(1, SampleFormat::S16, 1), &opts(None)).is_ok());
    assert!(encoder(&spec(96_000, SampleFormat::S16, 1), &opts(None)).is_ok());
    let what = unsupported(encoder(&spec(96_001, SampleFormat::S16, 1), &opts(None)));
    assert!(what.contains("96000"), "{what}");

    assert!(encoder(&spec(44100, SampleFormat::S16, 2), &opts(Some(8))).is_ok());
    let what = unsupported(encoder(&spec(44100, SampleFormat::S16, 2), &opts(Some(9))));
    assert!(what.contains("0 to 8"), "{what}");
}

#[test]
fn the_provisional_streaminfo_has_the_format_and_blank_totals() {
    let e = encoder(&spec(48000, SampleFormat::S24, 6), &opts(None)).unwrap();
    let b = StreamInfoBlock::parse(e.extradata()).unwrap();
    assert_eq!(
        b,
        StreamInfoBlock {
            min_block_size: 4096,
            max_block_size: 4096,
            min_frame_size: 0,
            max_frame_size: 0,
            sample_rate: 48000,
            channels: 6,
            bits_per_sample: 24,
            total_samples: 0,
            md5: [0; 16],
        }
    );
    let u8e = encoder(&spec(8000, SampleFormat::U8, 1), &opts(None)).unwrap();
    assert_eq!(
        StreamInfoBlock::parse(u8e.extradata()).unwrap().bits_per_sample,
        8
    );
}

#[test]
fn a_frame_of_another_spec_is_an_error() {
    let mut e = encoder(&spec(44100, SampleFormat::S16, 2), &opts(None)).unwrap();
    let mut f = AudioFrame::new(&spec(48000, SampleFormat::S16, 2));
    f.samples = Samples::S16(vec![0; 4]);
    assert!(matches!(e.send_frame(&f), Err(Error::Unsupported { .. })));
    let mut f = AudioFrame::new(&spec(44100, SampleFormat::S16, 1));
    f.samples = Samples::S16(vec![0; 4]);
    assert!(matches!(e.send_frame(&f), Err(Error::Unsupported { .. })));
    let mut f = AudioFrame::new(&spec(44100, SampleFormat::S24, 2));
    f.samples = Samples::S24(vec![0; 4]);
    assert!(matches!(e.send_frame(&f), Err(Error::Unsupported { .. })));
}
