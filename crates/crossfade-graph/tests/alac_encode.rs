//! Encoding ALAC into M4A through the engine: lossless, so the file decodes to the source's
//! samples, with its length, depth, layout and tags.

mod common;

use common::{job, sine_wav, spec};
use crossfade_core::{ChannelLayout, CodecId, Error, Limits, SampleFormat, Samples};
use crossfade_format::open_demuxer;
use crossfade_graph::{
    CancelToken, Event, Job, Pipeline, can_hold, decode_frames, run, validate, writable_extensions,
};
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

fn convert(io: &mut MemoryIo, j: &Job) -> Vec<Event> {
    let mut events = Vec::new();
    run(j, io, &Limits::default(), &CancelToken::new(), &mut |e| {
        events.push(e.clone())
    })
    .unwrap();
    events
}

#[test]
fn pcm_becomes_an_m4a_that_decodes_to_the_same_samples() {
    for (rate, format, layout) in [
        (44_100, SampleFormat::S16, ChannelLayout::stereo()),
        (96_000, SampleFormat::S24, ChannelLayout::mono()),
        (
            48_000,
            SampleFormat::S32,
            ChannelLayout::from_name("5.1").unwrap(),
        ),
    ] {
        let s = spec(rate, format, layout.clone());
        let frames = 4096 * 3 + 500;
        let mut io = MemoryIo::new();
        io.insert("in.wav", sine_wav(&s, frames).1);
        // Label the channels first: a plain multichannel WAV has none.
        let mut j = job("in.wav", "labelled.wav", None);
        j.outputs[0].audio.channels = layout
            .name()
            .map(|n| crossfade_graph::filter::parse_channels(n).unwrap());
        convert(&mut io, &j);
        let events = convert(&mut io, &job("labelled.wav", "out.m4a", None));
        let Some(Event::Started { plan }) = events.first() else {
            panic!("no start");
        };
        assert!(!plan.copy);
        assert_eq!(plan.output.codec, CodecId::Alac);
        assert_eq!(plan.notes, Vec::<String>::new());
        let stream = stream_of(&io, "out.m4a");
        assert_eq!(
            stream, plan.output,
            "{rate} {format:?}: the plan is what was written"
        );
        assert_eq!(stream.frames, Some(frames as u64));
        assert_eq!(
            (stream.audio.format(), stream.audio.layout()),
            (format, &layout)
        );
        assert!(
            decoded(&mut io, "out.m4a") == decoded(&mut io, "labelled.wav"),
            "{rate} {format:?}"
        );
        // And on from there, untouched or decoded again.
        let mut copy = job("out.m4a", "copy.m4a", None);
        copy.outputs[0].audio.codec = Some("copy".into());
        convert(&mut io, &copy);
        assert!(decoded(&mut io, "copy.m4a") == decoded(&mut io, "labelled.wav"));
    }
}

#[test]
fn the_plan_agrees_without_a_file_and_the_extension_is_writable() {
    assert!(writable_extensions().contains(&"m4a"));
    assert!(can_hold(crossfade_format::FormatId::Mp4, CodecId::Alac));
    assert!(!can_hold(crossfade_format::FormatId::Mp4, CodecId::Flac));
    let s = spec(44_100, SampleFormat::S16, ChannelLayout::stereo());
    let mut io = MemoryIo::new();
    io.insert("in.wav", sine_wav(&s, 5000).1);
    let mut j = job("in.wav", "out.m4a", None);
    j.outputs[0].audio.codec = Some("alac".into());
    let input = stream_of(&io, "in.wav");
    let planned = validate(&input, "out.m4a", &j.outputs[0].audio).unwrap();
    let p = Pipeline::open(&j, &mut io, &Limits::default()).unwrap();
    assert_eq!(planned.output, p.plan().output);
}

#[test]
fn what_alac_or_m4a_cannot_take_is_said_by_name() {
    let mut io = MemoryIo::new();
    let s16 = spec(44_100, SampleFormat::S16, ChannelLayout::stereo());
    io.insert("in.wav", sine_wav(&s16, 4800).1);
    let f32s = spec(44_100, SampleFormat::F32, ChannelLayout::stereo());
    io.insert("float.wav", sine_wav(&f32s, 4800).1);
    let u8s = spec(44_100, SampleFormat::U8, ChannelLayout::stereo());
    io.insert("eight.wav", sine_wav(&u8s, 4800).1);

    assert_eq!(
        refusal(&mut io, &job("float.wav", "out.m4a", None)),
        "alac cannot store floating-point samples; choose --sample-format s16, s24 or s32"
    );
    assert_eq!(
        refusal(&mut io, &job("eight.wav", "out.m4a", None)),
        "alac holds 16-, 20-, 24- and 32-bit audio, not 8-bit; choose --sample-format s16"
    );
    // Asked for, both become possible.
    assert!(
        Pipeline::open(
            &job("float.wav", "a.m4a", Some(SampleFormat::S24)),
            &mut io,
            &Limits::default()
        )
        .is_ok()
    );
    assert!(
        Pipeline::open(
            &job("eight.wav", "b.m4a", Some(SampleFormat::S16)),
            &mut io,
            &Limits::default()
        )
        .is_ok()
    );

    let mut j = job("in.wav", "out.m4a", None);
    j.outputs[0].audio.codec = Some("flac".into());
    assert_eq!(
        refusal(&mut io, &j),
        "codec flac cannot be written into mp4 (m4a holds alac)"
    );
    j.outputs[0].audio.codec = Some("copy".into());
    assert_eq!(
        refusal(&mut io, &j),
        "cannot copy pcm_s16le into mp4 (m4a holds alac)"
    );
    j = job("in.wav", "out.m4a", None);
    j.outputs[0].audio.compression_level = Some(5);
    assert_eq!(
        refusal(&mut io, &j),
        "--compression-level applies to flac only"
    );

    // ALAC has one layout per channel count: 5.1 with side surrounds.
    let back = spec(
        48_000,
        SampleFormat::S16,
        ChannelLayout::unpositioned(6).unwrap(),
    );
    io.insert("six.wav", sine_wav(&back, 4800).1);
    let mut j = job("six.wav", "back.wav", None);
    j.outputs[0].audio.channels =
        Some(crossfade_graph::filter::parse_channels("5.1-back").unwrap());
    convert(&mut io, &j);
    assert_eq!(
        refusal(&mut io, &job("back.wav", "out.m4a", None)),
        "alac cannot hold the layout 5.1-back (for 6 channels it has 5.1)"
    );
    // Unlabelled channels are taken as that layout, and the plan says so.
    let p = Pipeline::open(
        &job("six.wav", "six.m4a", None),
        &mut io,
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(p.plan().notes, ["6 unlabelled channels are written as 5.1"]);
    assert_eq!(io.get("out.m4a"), None);
}
