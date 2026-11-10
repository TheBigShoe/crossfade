mod common;

use common::{sine_wav, spec};
use crossfade_core::{ChannelLayout, CodecId, Error, Limits, SampleFormat};
use crossfade_graph::probe;
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
        r#"{"schema":1,"format":"wav","size_bytes":244,"streams":[{"index":0,"type":"audio","codec":"pcm_s16le","sample_format":"s16","sample_rate":8000,"channels":1,"layout":"mono","frames":100,"duration_seconds":0.0125}]}"#
    );
}
