mod common;

use common::{sine_wav, spec};
use crossfade_core::{
    ChannelLayout, CodecId, Error, Limits, SampleFormat, StreamInfo, Tags, TimeBase,
};
use crossfade_format::FormatId;
use crossfade_graph::{ProbeOptions, ProbeReport, probe, probe_with};
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
        loudness: None,
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
}

fn tone_wav(rate: u32, seconds: u32, amp: f64) -> Vec<u8> {
    let s = spec(rate, SampleFormat::S16, ChannelLayout::stereo());
    let mut pcm = Vec::new();
    for i in 0..rate * seconds {
        let v = (amp
            * 32767.0
            * (2.0 * std::f64::consts::PI * 1000.0 * f64::from(i) / f64::from(rate)).sin())
        .round() as i16;
        pcm.extend_from_slice(&v.to_le_bytes());
        pcm.extend_from_slice(&v.to_le_bytes());
    }
    common::wav_bytes(&s, &pcm)
}

#[test]
fn probe_with_loudness_reads_an_ebu_tone() {
    let mut io = MemoryIo::new();
    io.insert("a.wav", tone_wav(48_000, 20, 10f64.powf(-23.0 / 20.0)));
    let report = probe_with(
        "a.wav",
        &mut io,
        &Limits::default(),
        &ProbeOptions { loudness: true },
    )
    .unwrap();
    let l = report.loudness.expect("asked for");
    assert!((l.integrated_lufs.unwrap() + 23.0).abs() < 0.1, "{l:?}");
    assert!(l.true_peak_dbtp.unwrap() >= l.sample_peak_dbfs.unwrap());
    assert!((l.sample_peak_dbfs.unwrap() + 23.0).abs() < 0.01);
    assert!(l.range_lu.is_some());
    assert_eq!(report.document().loudness, Some(Box::new(l)));
}

#[test]
fn probe_with_no_options_is_probe_and_json_has_no_loudness_key() {
    let mut io = MemoryIo::new();
    io.insert("a.wav", tone_wav(8000, 1, 0.5));
    let plain = probe("a.wav", &mut io, &Limits::default()).unwrap();
    let with = probe_with(
        "a.wav",
        &mut io,
        &Limits::default(),
        &ProbeOptions::default(),
    )
    .unwrap();
    assert_eq!(plain, with);
    assert_eq!(plain.loudness, None);
    let json = serde_json::to_string(&plain.document()).unwrap();
    assert!(!json.contains("loudness"), "{json}");
}

#[test]
fn silence_gives_nulls_in_the_json() {
    let mut io = MemoryIo::new();
    io.insert("a.wav", tone_wav(48_000, 1, 0.0));
    let doc = probe_with(
        "a.wav",
        &mut io,
        &Limits::default(),
        &ProbeOptions { loudness: true },
    )
    .unwrap()
    .document();
    let json = serde_json::to_string(&doc).unwrap();
    assert!(
        json.ends_with(r#""loudness":{"integrated_lufs":null,"range_lu":null,"true_peak_dbtp":null,"sample_peak_dbfs":null}}"#),
        "{json}"
    );
    let back: crossfade_graph::ProbeDocument = serde_json::from_str(&json).unwrap();
    assert_eq!(back, doc);
}
