//! Ogg Opus stream copy through the engine: the copy reads back with the source's packets,
//! timeline, length and tags and decodes to the same samples; anything that would need an
//! Opus encoder is refused by name.

mod common;

use common::{job, sine_wav, spec};
use crossfade_core::{ChannelLayout, Error, Limits, Packet, SampleFormat, Samples, StreamInfo};
use crossfade_format::open_demuxer;
use crossfade_graph::{
    AudioSettings, CancelToken, Event, Job, Pipeline, decode_frames, run, validate,
};
use crossfade_opus::testing::ogg::{self as w, OggLayout};
use crossfade_testkit::memory::MemoryIo;
use std::io::Cursor;

const PRE_SKIP: u16 = 312;
const END_TRIM: i64 = 200;

/// SILK, hybrid and CELT, three seconds: the copy's pages are not the source's.
fn ogg() -> Vec<u8> {
    let mut audio = w::packets(9, true, 50, 50, 1);
    audio.extend(w::packets(15, true, 50, 70, 2));
    audio.extend(w::packets(31, true, 50, 90, 3));
    w::ogg_file(
        &w::opus_head(2, PRE_SKIP, 0, None),
        &w::opus_tags("some encoder", &["TITLE=Tone", "R128_TRACK_GAIN=-512"]),
        &audio,
        &OggLayout {
            end_trim: END_TRIM,
            ..OggLayout::default()
        },
    )
}

const FRAMES: u64 = 150 * 960 - PRE_SKIP as u64 - END_TRIM as u64;

fn copying(input: &str, output: &str) -> Job {
    let mut j = job(input, output, None);
    j.outputs[0].audio.codec = Some("copy".into());
    j
}

type Tags = Vec<(String, String)>;

fn read(bytes: &[u8]) -> (StreamInfo, Tags, Vec<Packet>) {
    let (_, mut d) =
        open_demuxer(Box::new(Cursor::new(bytes.to_vec())), &Limits::default()).unwrap();
    let mut packets = vec![];
    let mut p = Packet::default();
    while d.read_packet(&mut p).unwrap() {
        packets.push(p.clone());
    }
    let tags = d
        .tags()
        .iter()
        .map(|t| (t.key.to_string(), t.value.to_string()))
        .collect();
    (d.streams()[0].clone(), tags, packets)
}

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

fn refusal(io: &mut MemoryIo, j: &Job) -> String {
    match Pipeline::open(j, io, &Limits::default()) {
        Err(Error::Unsupported { what }) => what,
        Err(e) => panic!("wrong error {e:?}"),
        Ok(_) => panic!("opened"),
    }
}

#[test]
fn an_opus_copy_reads_back_and_decodes_as_its_source() {
    let source = ogg();
    for out in ["out.opus", "out.ogg", "out.oga"] {
        let mut io = MemoryIo::new();
        io.insert("in.opus", source.clone());
        let mut events = Vec::new();
        run(
            &copying("in.opus", out),
            &mut io,
            &Limits::default(),
            &CancelToken::new(),
            &mut |e| events.push(e.clone()),
        )
        .unwrap();
        let Some(Event::Started { plan }) = events.first() else {
            panic!("no start");
        };
        assert!(plan.copy);
        assert_eq!(events.last(), Some(&Event::Finished { frames: FRAMES }));

        let written = io.get(out).unwrap();
        let (want, got) = (read(&source), read(&written));
        assert_eq!(want.0.frames, Some(FRAMES));
        assert_eq!(want.2[0].pts, -i64::from(PRE_SKIP));
        assert_eq!(got.0, want.0, "{out}: stream");
        assert_eq!(got.1, want.1, "{out}: tags");
        assert!(got.2 == want.2, "{out}: packets");

        let (a, b) = (decoded(&mut io, "in.opus"), decoded(&mut io, out));
        assert_eq!(a.len() as u64, FRAMES * 2);
        assert!(a == b, "{out}: samples");
    }
}

#[test]
fn validate_accepts_the_copy_and_open_agrees() {
    let mut io = MemoryIo::new();
    io.insert("in.opus", ogg());
    let stream = read(&ogg()).0;
    let j = copying("in.opus", "out.opus");
    let planned = validate(&stream, "out.opus", &j.outputs[0].audio).unwrap();
    let p = Pipeline::open(&j, &mut io, &Limits::default()).unwrap();
    assert!(planned.copy);
    assert_eq!(planned.output, p.plan().output);
    assert_eq!(planned.output, stream);
}

#[test]
fn what_needs_an_opus_encoder_is_refused_by_name() {
    const NO_ENCODER: &str =
        "opus encoding is not available yet; --codec copy keeps an opus input as it is";
    let mut io = MemoryIo::new();
    io.insert("in.opus", ogg());
    let s = spec(48_000, SampleFormat::S16, ChannelLayout::stereo());
    io.insert("in.wav", sine_wav(&s, 4800).1);

    // `.opus` stands for Opus, from PCM and from Opus alike.
    for input in ["in.wav", "in.opus"] {
        let j = job(input, "out.opus", None);
        assert_eq!(refusal(&mut io, &j), NO_ENCODER, "{input}");
        assert!(!io.has_pending());
        assert_eq!(io.get("out.opus"), None);
    }
    // Asked for by name into `.ogg`, where the default would be FLAC.
    let mut j = job("in.wav", "out.ogg", None);
    j.outputs[0].audio.codec = Some("opus".into());
    assert_eq!(refusal(&mut io, &j), NO_ENCODER);
    let stream = read(&ogg()).0;
    let e = validate(&stream, "out.opus", &AudioSettings::default()).unwrap_err();
    assert_eq!(e.to_string(), format!("unsupported: {NO_ENCODER}"));

    // PCM is not Opus: a copy into `.opus` has nothing to copy.
    assert_eq!(
        refusal(&mut io, &copying("in.wav", "out.opus")),
        "cannot copy pcm_s16le into ogg (ogg holds flac or opus)"
    );
    // And Opus goes into no other container.
    assert_eq!(
        refusal(&mut io, &copying("in.opus", "out.flac")),
        "cannot copy opus into flac"
    );
}
