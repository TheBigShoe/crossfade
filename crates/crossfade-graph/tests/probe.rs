mod common;

use common::{sine_wav, spec};
use crossfade_core::{
    ChannelLayout, CodecId, Error, Limits, SampleFormat, StreamInfo, Tags, TimeBase,
};
use crossfade_format::FormatId;
use crossfade_graph::{ProbeReport, probe};
use crossfade_testkit::memory::MemoryIo;
use std::io::ErrorKind;

#[test]
fn reports_format_size_and_stream() {
    let s = spec(8000, SampleFormat::S16, ChannelLayout::mono());
    let (_, wav) = sine_wav(&s, 100);
    let mut io = MemoryIo::new();
    io.insert("a.wav", wav);
    let report = probe("a.wav", &mut io, &Limits::default()).unwrap();
    assert_eq!(report.format.name(), "wav");
    assert_eq!(report.size_bytes, Some(244));
    assert_eq!(report.streams.len(), 1);
    let st = &report.streams[0];
    assert_eq!(st.codec, CodecId::PcmS16Le);
    assert_eq!(st.audio, s);
    assert_eq!(st.frames, Some(100));
}

#[test]
fn junk_is_unrecognised() {
    let mut io = MemoryIo::new();
    io.insert("z.bin", vec![0; 64]);
    assert_eq!(
        probe("z.bin", &mut io, &Limits::default()).err(),
        Some(Error::UnrecognisedFormat)
    );
}

#[test]
fn missing_file_is_an_io_error() {
    let mut io = MemoryIo::new();
    assert!(matches!(
        probe("nope.wav", &mut io, &Limits::default()),
        Err(Error::Io {
            kind: ErrorKind::NotFound,
            ..
        })
    ));
}

#[test]
fn document_serialises_the_schema_1_field_order() {
    let s = spec(8000, SampleFormat::S16, ChannelLayout::mono());
    let (_, wav) = sine_wav(&s, 100);
    let mut io = MemoryIo::new();
    io.insert("a.wav", wav);
    let doc = probe("a.wav", &mut io, &Limits::default())
        .unwrap()
        .document();
    assert_eq!(doc.schema, 1);
    let json = serde_json::to_string(&doc).unwrap();
    assert_eq!(
        json,
        r#"{"schema":1,"format":"wav","size_bytes":244,"streams":[{"index":0,"type":"audio","codec":"pcm_s16le","sample_format":"s16","bits_per_raw_sample":null,"sample_rate":8000,"channels":1,"layout":"mono","frames":100,"duration_seconds":0.0125}],"tags":[]}"#
    );
}

#[test]
fn a_wav_has_no_tags() {
    let s = spec(8000, SampleFormat::S16, ChannelLayout::mono());
    let mut io = MemoryIo::new();
    io.insert("a.wav", sine_wav(&s, 10).1);
    let report = probe("a.wav", &mut io, &Limits::default()).unwrap();
    assert!(report.tags.is_empty());
}

#[test]
fn tags_and_raw_bits_reach_the_document_in_order() {
    let audio = spec(44100, SampleFormat::S24, ChannelLayout::stereo());
    let stream = StreamInfo {
        bits_per_raw_sample: Some(20),
        ..StreamInfo::new(0, CodecId::Flac, audio, TimeBase::new(1, 44100).unwrap())
    };
    let mut tags = Tags::new();
    tags.push("ARTIST", "A");
    tags.push("ARTIST", "B");
    tags.push("TITLE", "T");
    let doc = ProbeReport {
        format: FormatId::Flac,
        size_bytes: None,
        streams: vec![stream],
        tags,
    }
    .document();
    assert_eq!(doc.streams[0].bits_per_raw_sample, Some(20));
    let tags: Vec<_> = doc
        .tags
        .iter()
        .map(|t| (t.key.as_str(), t.value.as_str()))
        .collect();
    assert_eq!(tags, [("ARTIST", "A"), ("ARTIST", "B"), ("TITLE", "T")]);
    let json = serde_json::to_string(&doc).unwrap();
    assert!(json.contains(r#""bits_per_raw_sample":20"#), "{json}");
    assert!(json.ends_with(r#""tags":[{"key":"ARTIST","value":"A"},{"key":"ARTIST","value":"B"},{"key":"TITLE","value":"T"}]}"#), "{json}");
    let back: crossfade_graph::ProbeDocument = serde_json::from_str(&json).unwrap();
    assert_eq!(back, doc);
    let _ = ProbeTag {
        key: String::new(),
        value: String::new(),
    };
}
