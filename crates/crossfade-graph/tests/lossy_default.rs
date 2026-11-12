//! A lossy codec has no bit depth: with no sample format named, its float output is written
//! as 16-bit to every container, with a note, and dithered like any float to integer.

use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Error, SampleFormat, StreamInfo, TimeBase,
};
use crossfade_graph::{AudioSettings, Dither, describe, describe_concat, processing};

fn stream(codec: CodecId) -> StreamInfo {
    let audio = AudioSpec::new(44_100, SampleFormat::F32, ChannelLayout::stereo()).unwrap();
    StreamInfo {
        frames: Some(44_100),
        ..StreamInfo::new(0, codec, audio, TimeBase::new(1, 44_100).unwrap())
    }
}

fn with(f: impl FnOnce(&mut AudioSettings)) -> AudioSettings {
    let mut s = AudioSettings::default();
    f(&mut s);
    s
}

fn note(codec: &str) -> String {
    format!("{codec} has no bit depth: writing 16-bit; choose --sample-format to change")
}

#[test]
fn every_writable_container_gets_16_bit_and_a_note() {
    for codec in [CodecId::Mp3, CodecId::Vorbis, CodecId::Opus, CodecId::Aac] {
        for (out, written) in [
            ("o.wav", CodecId::PcmS16Le),
            ("o.aiff", CodecId::PcmS16Be),
            ("o.flac", CodecId::Flac),
            ("o.oga", CodecId::Flac),
        ] {
            let plan = describe(&stream(codec), out, &AudioSettings::default()).unwrap();
            let case = format!("{} -> {out}", codec.name());
            assert_eq!(plan.output.audio.format(), SampleFormat::S16, "{case}");
            assert_eq!(plan.output.codec, written, "{case}");
            assert_eq!(plan.output.bits_per_raw_sample, None, "{case}");
            assert_eq!(plan.notes, [note(codec.name())], "{case}");
        }
    }
}

#[test]
fn float_to_16_bit_is_dithered_unless_the_job_says_none() {
    let mp3 = stream(CodecId::Mp3);
    let chain = processing(&mp3, &AudioSettings::default())
        .unwrap()
        .unwrap();
    assert!(chain.filters.is_empty());
    assert_eq!(chain.dither, Dither::Tpdf);
    let none = with(|s| s.dither = Dither::None);
    assert!(processing(&mp3, &none).unwrap().is_none());
}

#[test]
fn a_named_sample_format_is_honoured_without_a_note() {
    let mp3 = stream(CodecId::Mp3);
    for (format, out) in [
        (SampleFormat::F32, "o.wav"),
        (SampleFormat::F64, "o.aiff"),
        (SampleFormat::S24, "o.flac"),
        (SampleFormat::S16, "o.wav"),
    ] {
        let plan = describe(&mp3, out, &with(|s| s.sample_format = Some(format))).unwrap();
        assert_eq!(plan.output.audio.format(), format, "{format:?} {out}");
        assert!(plan.notes.is_empty(), "{format:?} {out}");
    }
    let to_f32 = with(|s| s.sample_format = Some(SampleFormat::F32));
    assert!(
        processing(&mp3, &to_f32).unwrap().is_none(),
        "nothing to quantise"
    );
}

#[test]
fn float_to_flac_is_still_refused_when_asked_for_or_when_the_source_is_float_pcm() {
    let refusal = Err(Error::Unsupported {
        what: "flac cannot store floating-point samples; choose --sample-format s16, s24 or s32"
            .into(),
    });
    let to_f32 = with(|s| s.sample_format = Some(SampleFormat::F32));
    assert_eq!(describe(&stream(CodecId::Mp3), "o.flac", &to_f32), refusal);
    let float_wav = stream(CodecId::PcmF32Le);
    assert_eq!(
        describe(&float_wav, "o.flac", &AudioSettings::default()),
        refusal
    );
    let plan = describe(&float_wav, "o.wav", &AudioSettings::default()).unwrap();
    assert_eq!(plan.output.audio.format(), SampleFormat::F32);
    assert!(plan.notes.is_empty());
}

#[test]
fn a_join_that_starts_lossy_is_16_bit_too() {
    let inputs = [stream(CodecId::Mp3), stream(CodecId::Mp3)];
    let plan = describe_concat(&inputs, "o.flac", &AudioSettings::default()).unwrap();
    assert_eq!(plan.output.audio.format(), SampleFormat::S16);
    assert_eq!(plan.notes, [note("mp3")]);
}

#[test]
fn writing_mp3_is_still_refused_by_its_extension() {
    let what = match describe(
        &stream(CodecId::PcmS16Le),
        "o.mp3",
        &AudioSettings::default(),
    ) {
        Err(Error::Unsupported { what }) => what,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        what,
        "output extension \"mp3\" of o.mp3 (can write .wav, .flac, .aiff/.aif/.aifc, .oga/.ogg)"
    );
}
