//! Encoding Opus through the engine: any input becomes 48 kHz float for the encoder, the file
//! reads back with the audible length and decodes to the source's sound.

mod common;

use common::{job, sine_wav, spec};
use crossfade_core::{ChannelLayout, CodecId, Error, Limits, SampleFormat, Samples};
use crossfade_format::open_demuxer;
use crossfade_graph::{CancelToken, Event, Job, Pipeline, decode_frames, describe, run, validate};
use crossfade_testkit::memory::MemoryIo;
use std::io::Cursor;

fn decoded(io: &mut MemoryIo, path: &str) -> Vec<f64> {
    let mut out = vec![];
    decode_frames(
        path,
        io,
        &Limits::default(),
        &CancelToken::new(),
        &mut |_, f| {
            let Samples::F64(v) = &f.samples else {
                panic!("decode_frames gives f64")
            };
            out.extend_from_slice(v);
            Ok(())
        },
    )
    .unwrap();
    out
}

fn stream_of(io: &MemoryIo, path: &str) -> crossfade_core::StreamInfo {
    let bytes = io.get(path).unwrap();
    let (_, d) = open_demuxer(Box::new(Cursor::new(bytes)), &Limits::default()).unwrap();
    d.streams()[0].clone()
}

fn refusal(io: &mut MemoryIo, j: &Job) -> String {
    match Pipeline::open(j, io, &Limits::default()) {
        Err(Error::Unsupported { what }) => what,
        Err(e) => panic!("wrong error {e:?}"),
        Ok(_) => panic!("opened"),
    }
}

/// How far `got` (interleaved, `ch` channels at 48 kHz) is from the 440 Hz sine of amplitude
/// 0.5 that `sine_wav` renders, as a signal-to-noise ratio.
fn snr_against_the_sine(got: &[f64], ch: usize) -> f64 {
    let (mut sig, mut err) = (0.0, 0.0);
    for (i, frame) in got.chunks(ch).enumerate().skip(4800) {
        let want = 0.5 * (std::f64::consts::TAU * 440.0 * i as f64 / 48_000.0).sin();
        for &g in frame {
            sig += want * want;
            err += (want - g) * (want - g);
        }
    }
    sig / err
}

#[test]
fn any_pcm_becomes_ogg_opus_of_its_length_at_48_khz() {
    for (rate, format, layout, out) in [
        (
            48_000,
            SampleFormat::S16,
            ChannelLayout::stereo(),
            "out.opus",
        ),
        (44_100, SampleFormat::S24, ChannelLayout::mono(), "out.opus"),
        (
            96_000,
            SampleFormat::F32,
            ChannelLayout::stereo(),
            "out.ogg",
        ),
    ] {
        let s = spec(rate, format, layout);
        let ch = usize::from(s.layout().channels());
        let frames_in = rate as usize * 2;
        let mut io = MemoryIo::new();
        io.insert("in.wav", sine_wav(&s, frames_in).1);
        let mut j = job("in.wav", out, None);
        if out.ends_with(".ogg") {
            // `.ogg` is FLAC unless Opus is named; `.opus` names it.
            j.outputs[0].audio.codec = Some("opus".into());
        }
        let mut events = Vec::new();
        run(
            &j,
            &mut io,
            &Limits::default(),
            &CancelToken::new(),
            &mut |e| events.push(e.clone()),
        )
        .unwrap();
        let Some(Event::Started { plan }) = events.first() else {
            panic!("no start");
        };
        assert!(!plan.copy);
        assert_eq!(plan.output.codec, CodecId::Opus);
        assert_eq!(plan.output.frames, Some(96_000));
        assert_eq!(
            plan.notes,
            Vec::<String>::new(),
            "{rate}: no bit depth is spoken of"
        );

        let stream = stream_of(&io, out);
        assert_eq!(stream.codec, CodecId::Opus);
        assert_eq!(
            (stream.audio.sample_rate(), stream.frames),
            (48_000, Some(96_000))
        );
        assert_eq!(usize::from(stream.audio.layout().channels()), ch);
        let got = decoded(&mut io, out);
        assert_eq!(
            got.len(),
            96_000 * ch,
            "{rate}: priming and padding are trimmed"
        );
        let snr = snr_against_the_sine(&got, ch);
        assert!(snr > 300.0, "{rate} Hz {format:?}: snr {snr:.0}");
    }
}

#[test]
fn the_plan_is_the_same_without_touching_a_file() {
    let s = spec(44_100, SampleFormat::S16, ChannelLayout::stereo());
    let mut io = MemoryIo::new();
    io.insert("in.wav", sine_wav(&s, 44_100).1);
    let j = job("in.wav", "out.opus", None);
    let input = stream_of(&io, "in.wav");
    let described = describe(&input, "out.opus", &j.outputs[0].audio).unwrap();
    let validated = validate(&input, "out.opus", &j.outputs[0].audio).unwrap();
    let p = Pipeline::open(&j, &mut io, &Limits::default()).unwrap();
    assert_eq!(described.output, p.plan().output);
    assert_eq!(validated.output, p.plan().output);
    assert_eq!(p.plan().output.audio.format(), SampleFormat::F32);
}

#[test]
fn opus_from_opus_and_what_opus_cannot_take_are_said_by_name() {
    let s = spec(48_000, SampleFormat::S16, ChannelLayout::stereo());
    let mut io = MemoryIo::new();
    io.insert("in.wav", sine_wav(&s, 48_000).1);
    run(
        &job("in.wav", "a.opus", None),
        &mut io,
        &Limits::default(),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap();
    // Re-encoding is allowed (a copy needs --codec copy), and says nothing of bit depth.
    run(
        &job("a.opus", "b.opus", None),
        &mut io,
        &Limits::default(),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(stream_of(&io, "b.opus").frames, Some(48_000));

    let mut j = job("in.wav", "out.opus", Some(SampleFormat::S16));
    assert_eq!(
        refusal(&mut io, &j),
        "opus has no sample format; leave out --sample-format"
    );
    j = job("in.wav", "out.opus", None);
    j.outputs[0].audio.sample_rate = Some(44_100);
    assert_eq!(
        refusal(&mut io, &j),
        "opus is written at 48000 Hz; leave out --sample-rate"
    );
    j.outputs[0].audio.sample_rate = Some(48_000);
    assert!(Pipeline::open(&j, &mut io, &Limits::default()).is_ok());
    j = job("in.wav", "out.opus", None);
    j.outputs[0].audio.compression_level = Some(5);
    assert_eq!(
        refusal(&mut io, &j),
        "--compression-level applies to flac only"
    );
    j = job("in.wav", "out.wav", None);
    j.outputs[0].audio.codec = Some("opus".into());
    assert_eq!(
        refusal(&mut io, &j),
        "codec opus cannot be written into wav"
    );

    let surround = spec(
        48_000,
        SampleFormat::S16,
        ChannelLayout::from_name("5.1").unwrap(),
    );
    io.insert("six.wav", sine_wav(&surround, 4800).1);
    assert_eq!(
        refusal(&mut io, &job("six.wav", "out.opus", None)),
        "opus encoding takes mono or stereo, not 6 channels; choose --channels stereo"
    );
    assert_eq!(io.get("out.opus"), None);
}
