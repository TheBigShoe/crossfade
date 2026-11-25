//! Writing Opus: what the plan says, what it refuses and what the job runs end to end. Opus is
//! always 48 kHz, float, mono or stereo; `.opus` is Ogg Opus and `--codec opus` puts Opus in
//! `.ogg`/`.oga` too.
mod common;

use common::{job, sine_wav, spec};
use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Error, Limits, SampleFormat, StreamInfo, TimeBase,
};
use crossfade_format::FormatId;
use crossfade_graph::filter::Channels;
use crossfade_graph::{
    AudioSettings, CancelToken, describe, describe_concat, probe, run, validate, validate_concat,
};
use crossfade_opus::OpusHead;
use crossfade_testkit::memory::MemoryIo;

fn stream(codec: CodecId, rate: u32, format: SampleFormat, layout: ChannelLayout) -> StreamInfo {
    let audio = AudioSpec::new(rate, format, layout).unwrap();
    StreamInfo {
        frames: Some(u64::from(rate)),
        ..StreamInfo::new(0, codec, audio, TimeBase::new(1, i64::from(rate)).unwrap())
    }
}

fn pcm(rate: u32, layout: ChannelLayout) -> StreamInfo {
    stream(CodecId::PcmS16Le, rate, SampleFormat::S16, layout)
}

fn with(f: impl FnOnce(&mut AudioSettings)) -> AudioSettings {
    let mut s = AudioSettings::default();
    f(&mut s);
    s
}

fn refusal(r: Result<impl Sized, Error>) -> String {
    match r {
        Err(Error::Unsupported { what }) => what,
        Err(e) => panic!("wrong error {e:?}"),
        Ok(_) => panic!("expected a refusal"),
    }
}

#[test]
fn opus_is_written_float_at_48_khz_into_ogg_with_its_header_known_up_front() {
    let plan = describe(
        &pcm(48_000, ChannelLayout::stereo()),
        "o.opus",
        &AudioSettings::default(),
    )
    .unwrap();
    assert_eq!(plan.output_format, FormatId::Ogg);
    assert_eq!(plan.output.codec, CodecId::Opus);
    assert_eq!(plan.output.audio.format(), SampleFormat::F32);
    assert_eq!(plan.output.audio.sample_rate(), 48_000);
    assert_eq!(plan.output.frames, Some(48_000));
    assert!(plan.notes.is_empty(), "{:?}", plan.notes);
    let head = OpusHead::parse(&plan.output.extradata).unwrap();
    assert_eq!((head.channels, head.pre_skip), (2, 312));
    assert_eq!(head.input_sample_rate, 48_000);
}

#[test]
fn another_rate_is_resampled_with_a_note_and_its_rate_recorded_in_the_header() {
    let plan = describe(
        &pcm(44_100, ChannelLayout::mono()),
        "o.opus",
        &AudioSettings::default(),
    )
    .unwrap();
    assert_eq!(plan.output.audio.sample_rate(), 48_000);
    assert_eq!(plan.output.frames, Some(48_000));
    assert_eq!(
        plan.notes,
        ["opus is always 48 kHz: resampling from 44100 Hz"]
    );
    let head = OpusHead::parse(&plan.output.extradata).unwrap();
    assert_eq!((head.channels, head.input_sample_rate), (1, 44_100));
}

#[test]
fn a_lossy_source_has_no_sixteen_bit_note_for_a_lossy_output() {
    let mp3 = stream(
        CodecId::Mp3,
        48_000,
        SampleFormat::F32,
        ChannelLayout::stereo(),
    );
    let plan = describe(&mp3, "o.opus", &AudioSettings::default()).unwrap();
    assert_eq!(plan.output.audio.format(), SampleFormat::F32);
    assert!(plan.notes.is_empty(), "{:?}", plan.notes);
}

#[test]
fn an_explicit_rate_other_than_48_khz_is_refused_and_48_khz_is_fine() {
    let input = pcm(44_100, ChannelLayout::stereo());
    let w = refusal(describe(
        &input,
        "o.opus",
        &with(|s| s.sample_rate = Some(44_100)),
    ));
    assert_eq!(
        w,
        "opus is always 48 kHz: --sample-rate 44100 cannot be written (use 48000 or leave it out)"
    );
    let plan = describe(&input, "o.opus", &with(|s| s.sample_rate = Some(48_000))).unwrap();
    assert_eq!(plan.output.audio.sample_rate(), 48_000);
}

#[test]
fn a_sample_format_is_refused_because_opus_has_no_bit_depth() {
    let w = refusal(describe(
        &pcm(48_000, ChannelLayout::stereo()),
        "o.opus",
        &with(|s| s.sample_format = Some(SampleFormat::S16)),
    ));
    assert_eq!(
        w,
        "opus has no bit depth: leave out --sample-format (it is always coded from 32-bit float)"
    );
}

#[test]
fn the_codec_opus_goes_into_ogg_and_oga_and_only_dot_opus_defaults_to_it() {
    let input = pcm(48_000, ChannelLayout::stereo());
    for out in ["o.ogg", "o.oga", "o.opus"] {
        let plan = describe(&input, out, &with(|s| s.codec = Some("opus".into()))).unwrap();
        assert_eq!(plan.output.codec, CodecId::Opus, "{out}");
        assert_eq!(plan.output_format, FormatId::Ogg, "{out}");
    }
    for out in ["o.ogg", "o.oga"] {
        let plan = describe(&input, out, &AudioSettings::default()).unwrap();
        assert_eq!(plan.output.codec, CodecId::Flac, "{out}");
    }
    assert_eq!(
        refusal(describe(
            &input,
            "o.wav",
            &with(|s| s.codec = Some("opus".into()))
        )),
        "codec opus cannot be written into wav"
    );
    assert_eq!(
        refusal(describe(
            &input,
            "o.opus",
            &with(|s| s.codec = Some("flac".into()))
        )),
        "codec flac cannot be written into .opus (an .opus file holds opus; use .ogg or .oga for flac)"
    );
}

#[test]
fn a_bitrate_means_something_to_opus_only_and_has_a_range() {
    let input = pcm(48_000, ChannelLayout::stereo());
    for (out, codec) in [("o.flac", None), ("o.wav", None), ("o.m4a", None)] {
        let w = refusal(describe(
            &input,
            out,
            &with(|s| {
                s.codec = codec;
                s.bitrate = Some(128_000);
            }),
        ));
        assert_eq!(w, "--bitrate applies to opus only", "{out}");
    }
    let flac = with(|s| s.constant_bitrate = true);
    assert_eq!(
        refusal(describe(&input, "o.flac", &flac)),
        "--cbr applies to opus only"
    );
    let copy = with(|s| {
        s.codec = Some("copy".into());
        s.bitrate = Some(64_000);
    });
    assert_eq!(
        refusal(describe(&input, "o.wav", &copy)),
        "--codec copy cannot take a bitrate"
    );
    for (bitrate, ok) in [
        (11_999, false),
        (12_000, true),
        (512_000, true),
        (512_001, false),
    ] {
        let r = describe(&input, "o.opus", &with(|s| s.bitrate = Some(bitrate)));
        assert_eq!(r.is_ok(), ok, "{bitrate}: {r:?}");
        if !ok {
            let w = refusal(r);
            assert!(
                w.contains(&format!("--bitrate {bitrate}"))
                    && w.contains("12000 to 512000 b/s (6 to 256 kb/s per channel)"),
                "{w}"
            );
        }
    }
    let mono = pcm(48_000, ChannelLayout::mono());
    assert!(describe(&mono, "o.opus", &with(|s| s.bitrate = Some(256_000))).is_ok());
    assert!(describe(&mono, "o.opus", &with(|s| s.bitrate = Some(256_001))).is_err());
}

#[test]
fn surround_is_refused_with_the_advice_that_works() {
    let five_one = pcm(48_000, ChannelLayout::from_name("5.1").unwrap());
    let w = refusal(describe(&five_one, "o.opus", &AudioSettings::default()));
    assert_eq!(
        w,
        "surround opus is not written yet (6 channels): use --channels stereo to convert"
    );
    let plan = describe(
        &five_one,
        "o.opus",
        &with(|s| s.channels = Some(Channels::Layout(ChannelLayout::stereo()))),
    )
    .unwrap();
    assert_eq!(plan.output.audio.layout().channels(), 2);
    let head = OpusHead::parse(&plan.output.extradata).unwrap();
    assert_eq!(head.channels, 2);
}

#[test]
fn validate_builds_the_encoder_and_the_muxer() {
    let plan = validate(
        &pcm(44_100, ChannelLayout::stereo()),
        "o.opus",
        &with(|s| s.bitrate = Some(64_000)),
    )
    .unwrap();
    assert_eq!(plan.output.codec, CodecId::Opus);
}

#[test]
fn a_join_into_opus_conforms_each_input_to_48_khz_with_notes() {
    let inputs = [
        pcm(44_100, ChannelLayout::stereo()),
        pcm(48_000, ChannelLayout::stereo()),
        pcm(22_050, ChannelLayout::stereo()),
    ];
    let plan = describe_concat(&inputs, "o.opus", &AudioSettings::default()).unwrap();
    assert_eq!(plan.output.audio.sample_rate(), 48_000);
    assert_eq!(plan.output.audio.format(), SampleFormat::F32);
    assert_eq!(plan.output.frames, Some(48_000 + 48_000 + 96_000));
    assert_eq!(
        plan.notes,
        [
            "opus is always 48 kHz: resampling from 44100 Hz",
            "opus is always 48 kHz: resampling from 22050 Hz"
        ]
    );
    let head = OpusHead::parse(&plan.output.extradata).unwrap();
    assert_eq!(head.input_sample_rate, 44_100);
    validate_concat(&inputs, "o.opus", &AudioSettings::default()).unwrap();
}

fn run_job(io: &mut MemoryIo, audio: AudioSettings) -> Result<(), Error> {
    let mut j = job("in.wav", "out.opus", None);
    j.outputs[0].audio = audio;
    run(&j, io, &Limits::default(), &CancelToken::new(), &mut |_| {})
}

#[test]
fn a_job_writes_an_opus_file_of_the_input_length_at_48_khz() {
    for (rate, frames) in [(44_100u32, 20_000usize), (48_000, 20_000), (48_000, 100)] {
        let mut io = MemoryIo::new();
        io.insert(
            "in.wav",
            sine_wav(
                &spec(rate, SampleFormat::S16, ChannelLayout::stereo()),
                frames,
            )
            .1,
        );
        run_job(&mut io, AudioSettings::default()).unwrap();
        let out = probe("out.opus", &mut io, &Limits::default()).unwrap();
        let s = &out.streams[0];
        assert_eq!(s.codec, CodecId::Opus);
        assert_eq!(s.audio.sample_rate(), 48_000);
        let want = (frames as u64 * 48_000).div_ceil(u64::from(rate));
        let got = s.frames.unwrap();
        // The resampler's own length rule: within one frame of the exact ratio.
        assert!(got.abs_diff(want) <= 1, "{rate} {frames}: {got} vs {want}");
    }
}
