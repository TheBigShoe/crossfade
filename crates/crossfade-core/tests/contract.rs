//! The engine contract: codec ids, packets, frames, errors and the four
//! pipeline traits.
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Decoder, Demuxer, Encoder, Error, Muxer, Packet,
    SampleFormat, Samples,
};

const FORMATS: [SampleFormat; 6] = [
    SampleFormat::U8,
    SampleFormat::S16,
    SampleFormat::S24,
    SampleFormat::S32,
    SampleFormat::F32,
    SampleFormat::F64,
];

const CODECS: [CodecId; 6] = [
    CodecId::PcmU8,
    CodecId::PcmS16Le,
    CodecId::PcmS24Le,
    CodecId::PcmS32Le,
    CodecId::PcmF32Le,
    CodecId::PcmF64Le,
];

#[test]
fn sample_format_names_round_trip() {
    let names = ["u8", "s16", "s24", "s32", "f32", "f64"];
    for (f, n) in FORMATS.into_iter().zip(names) {
        assert_eq!(f.name(), n);
        assert_eq!(SampleFormat::from_name(n), Some(f));
    }
    assert_eq!(SampleFormat::from_name("banana"), None);
    assert_eq!(SampleFormat::from_name("S16"), None);
    assert_eq!(SampleFormat::from_name(""), None);
}

#[test]
fn codec_names_follow_ffmpeg_and_round_trip() {
    let names = [
        "pcm_u8",
        "pcm_s16le",
        "pcm_s24le",
        "pcm_s32le",
        "pcm_f32le",
        "pcm_f64le",
    ];
    for (c, n) in CODECS.into_iter().zip(names) {
        assert_eq!(c.name(), n);
        assert_eq!(CodecId::from_name(n), Some(c));
    }
    assert_eq!(CodecId::from_name("mp3"), None);
    assert_eq!(CodecId::from_name(""), None);
}

#[test]
fn pcm_for_and_pcm_sample_format_are_inverse() {
    for f in FORMATS {
        assert_eq!(CodecId::pcm_for(f).pcm_sample_format(), Some(f));
    }
    for c in CODECS {
        let f = c.pcm_sample_format().unwrap();
        assert_eq!(CodecId::pcm_for(f), c);
    }
}

#[test]
fn samples_report_format_and_length() {
    for f in FORMATS {
        let s = Samples::new(f);
        assert_eq!(s.format(), f);
        assert_eq!(s.len(), 0);
        assert!(s.is_empty());
    }
    let s = Samples::S16(vec![1, 2, 3]);
    assert_eq!(s.format(), SampleFormat::S16);
    assert_eq!(s.len(), 3);
    assert!(!s.is_empty());
    assert_eq!(Samples::S24(vec![0; 4]).format(), SampleFormat::S24);
    assert_eq!(Samples::F64(vec![0.0; 2]).len(), 2);
}

#[test]
fn samples_clear_keeps_capacity() {
    let mut s = Samples::F32(Vec::with_capacity(64));
    let Samples::F32(v) = &mut s else {
        unreachable!()
    };
    v.extend([0.5; 10]);
    s.clear();
    assert!(s.is_empty());
    let Samples::F32(v) = &s else { unreachable!() };
    assert!(v.capacity() >= 64);
}

#[test]
fn audio_frame_counts_frames_not_samples() {
    let mono = AudioSpec::new(8000, SampleFormat::S16, ChannelLayout::mono()).unwrap();
    let mut f = AudioFrame::new(&mono);
    assert_eq!((f.frames(), f.pts, f.sample_rate), (0, 0, 8000));
    f.samples = Samples::S16(vec![0; 5]);
    assert_eq!(f.frames(), 5);
    assert_eq!(f.spec(), mono);

    let stereo = AudioSpec::new(44100, SampleFormat::F32, ChannelLayout::stereo()).unwrap();
    let mut f = AudioFrame::new(&stereo);
    f.samples = Samples::F32(vec![0.0; 6]);
    assert_eq!(f.frames(), 3);
    assert_eq!(f.spec(), stereo);
}

#[test]
fn packet_default_is_empty() {
    let p = Packet::default();
    assert_eq!((p.stream, p.pts, p.duration), (0, 0, 0));
    assert!(p.data.is_empty());
}

#[test]
fn io_error_keeps_kind_and_message() {
    let e = Error::from(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no such thing",
    ));
    match &e {
        Error::Io { kind, message } => {
            assert_eq!(*kind, std::io::ErrorKind::NotFound);
            assert!(message.contains("no such thing"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(e.clone(), e);
}

#[test]
fn new_error_display_strings() {
    assert_eq!(
        Error::UnexpectedEof { offset: 44 }.to_string(),
        "unexpected end of input at byte 44"
    );
    assert_eq!(Error::UnrecognisedFormat.to_string(), "unrecognised format");
    // The remaining variants must say what they carry.
    assert!(
        Error::Malformed {
            what: "bad fmt".into()
        }
        .to_string()
        .contains("bad fmt")
    );
    assert!(
        Error::Unsupported {
            what: "adpcm".into()
        }
        .to_string()
        .contains("adpcm")
    );
    assert!(!Error::Cancelled.to_string().is_empty());
}

#[test]
fn traits_are_object_safe() {
    fn _boxed(_: Box<dyn Demuxer>, _: Box<dyn Muxer>, _: Box<dyn Decoder>, _: Box<dyn Encoder>) {}
}
