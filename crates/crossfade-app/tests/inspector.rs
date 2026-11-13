use crossfade_app::inspector::{Section, depth, sections};
use crossfade_graph::{ProbeDocument, ProbeStream, ProbeTag};
use std::collections::BTreeSet;

fn stream(index: u32) -> ProbeStream {
    ProbeStream {
        index,
        kind: "audio".into(),
        codec: "pcm_u8".into(),
        sample_format: "u8".into(),
        bits_per_raw_sample: None,
        sample_rate: 8000,
        channels: 1,
        layout: "mono".into(),
        frames: None,
        duration_seconds: None,
    }
}

fn doc(size: Option<u64>, streams: Vec<ProbeStream>) -> ProbeDocument {
    ProbeDocument {
        schema: 1,
        format: "wav".into(),
        size_bytes: size,
        streams,
        tags: vec![],
        loudness: None,
    }
}

fn mono_u8_unknown_length() -> ProbeDocument {
    doc(None, vec![stream(0)])
}

fn stereo_s24_96k() -> ProbeDocument {
    let mut s = stream(0);
    s.codec = "pcm_s24le".into();
    s.sample_format = "s24".into();
    s.sample_rate = 96_000;
    s.channels = 2;
    s.layout = "stereo".into();
    s.frames = Some(6_000_000);
    s.duration_seconds = Some(62.5);
    doc(Some(36_000_044), vec![s])
}

fn tagged_20_bit() -> ProbeDocument {
    let mut d = stereo_s24_96k();
    d.streams[0].bits_per_raw_sample = Some(20);
    d.tags = [
        ("TITLE", "A Title"),
        ("ARTIST", "One"),
        ("ARTIST", "Two"),
        ("TRACKNUMBER", "3"),
    ]
    .map(|(key, value)| ProbeTag {
        key: key.into(),
        value: value.into(),
    })
    .to_vec();
    d
}

fn render(sections: &[Section]) -> String {
    let mut out = String::new();
    for s in sections {
        out.push_str(&format!("[{}]\n", s.title));
        for r in &s.rows {
            out.push_str(&format!("  {} ({}): {}\n", r.label, r.id, r.value));
        }
    }
    out
}

fn value(doc: &ProbeDocument, field: &str) -> String {
    sections(doc)
        .iter()
        .flat_map(|s| &s.rows)
        .find(|r| r.field == field)
        .unwrap_or_else(|| panic!("no row for {field}"))
        .value
        .clone()
}

fn raw(doc: &ProbeDocument, field: &str) -> String {
    sections(doc)
        .iter()
        .flat_map(|s| &s.rows)
        .find(|r| r.field == field)
        .unwrap_or_else(|| panic!("no row for {field}"))
        .raw
        .clone()
}

fn keys(v: &serde_json::Value) -> BTreeSet<String> {
    v.as_object()
        .unwrap()
        .iter()
        .filter(|(_, v)| !v.is_array() && !v.is_object())
        .map(|(k, _)| k.clone())
        .collect()
}

fn fields(s: &Section) -> BTreeSet<String> {
    s.rows.iter().map(|r| r.field.to_owned()).collect()
}

// A field added to `probe --json` without an inspector row must fail here.
#[test]
fn every_json_leaf_has_a_row_and_no_row_is_invented() {
    let d = stereo_s24_96k();
    let json = serde_json::to_value(&d).unwrap();
    let shown = sections(&d);
    assert_eq!(shown.len(), 2);

    let mut file_keys = keys(&json);
    file_keys.remove("schema");
    assert_eq!(fields(&shown[0]), file_keys);

    let stream_json = &json["streams"][0];
    // A null raw-bits value has no row: the row appears only when the value is present.
    let mut stream_keys = keys(stream_json);
    stream_keys.remove("bits_per_raw_sample");
    assert_eq!(fields(&shown[1]), stream_keys);
    assert!(shown.iter().all(|s| s.title != "Tags"));
}

#[test]
fn raw_bits_and_tags_each_have_a_row_when_present() {
    let d = tagged_20_bit();
    let json = serde_json::to_value(&d).unwrap();
    let shown = sections(&d);
    assert_eq!(shown.len(), 3);
    // Every scalar stream key, including the raw bits now that it is set.
    assert_eq!(fields(&shown[1]), keys(&json["streams"][0]));
    assert!(fields(&shown[1]).contains("bits_per_raw_sample"));
    assert_eq!(value(&d, "bits_per_raw_sample"), "20-bit");
    // The array key `tags` is shown as its own section, one row per entry.
    assert!(json["tags"].is_array());
    let tags = &shown[2];
    assert_eq!(tags.title, "Tags");
    assert!(tags.rows.iter().all(|r| r.field == "tags"));
    let rows: Vec<(String, String)> = tags
        .rows
        .iter()
        .map(|r| (r.label.to_string(), r.value.clone()))
        .collect();
    let want = [
        ("Title", "A Title"),
        ("Artist", "One"),
        ("Artist", "Two"),
        ("Track number", "3"),
    ]
    .map(|(l, v)| (l.to_owned(), v.to_owned()));
    assert_eq!(rows, want);
}

#[test]
fn a_document_without_tags_or_raw_bits_shows_neither() {
    let d = mono_u8_unknown_length();
    let shown = sections(&d);
    assert_eq!(shown.len(), 2);
    assert!(
        shown[1]
            .rows
            .iter()
            .all(|r| r.field != "bits_per_raw_sample")
    );
}

#[test]
fn tagged_20_bit_snapshot() {
    insta::assert_snapshot!(render(&sections(&tagged_20_bit())));
}

#[test]
fn one_section_per_stream_after_the_file() {
    let d = doc(Some(10), vec![stream(0), stream(1)]);
    let titles: Vec<_> = sections(&d).into_iter().map(|s| s.title).collect();
    assert_eq!(titles, ["File", "Stream 0", "Stream 1"]);
}

#[test]
fn mono_u8_unknown_length_snapshot() {
    insta::assert_snapshot!(render(&sections(&mono_u8_unknown_length())));
}

#[test]
fn stereo_s24_96k_snapshot() {
    insta::assert_snapshot!(render(&sections(&stereo_s24_96k())));
}

#[test]
fn sizes_are_exact_when_small_and_scaled_when_large() {
    let size = |n| value(&doc(Some(n), vec![]), "size_bytes");
    assert_eq!(size(0), "0 bytes");
    assert_eq!(size(1), "1 byte");
    assert_eq!(size(999), "999 bytes");
    assert_eq!(size(1_400), "1.4 kB");
    assert_eq!(size(1_400_000), "1.4 MB");
    assert_eq!(size(2_500_000_000), "2.5 GB");
    // Rounding up must not print "1000.0 kB".
    assert_eq!(size(999_950), "1.0 MB");
}

#[test]
fn sample_rates_trim_trailing_zeros() {
    let rate = |hz| {
        let mut s = stream(0);
        s.sample_rate = hz;
        value(&doc(None, vec![s]), "sample_rate")
    };
    assert_eq!(rate(44_100), "44.1 kHz");
    assert_eq!(rate(48_000), "48 kHz");
    assert_eq!(rate(11_025), "11.025 kHz");
    assert_eq!(rate(500), "500 Hz");
}

#[test]
fn durations_and_frames_are_readable() {
    let with = |frames, secs| {
        let mut s = stream(0);
        s.frames = Some(frames);
        s.duration_seconds = Some(secs);
        doc(None, vec![s])
    };
    assert_eq!(
        value(&with(1_234_567, 62.25), "duration_seconds"),
        "1:02.250"
    );
    assert_eq!(value(&with(1_234_567, 62.25), "frames"), "1,234,567");
    assert_eq!(value(&with(5, 0.0), "duration_seconds"), "0:00.000");
    assert_eq!(value(&with(5, 3723.5), "duration_seconds"), "1:02:03.500");
    assert_eq!(value(&with(999, 59.9996), "duration_seconds"), "1:00.000");
}

#[test]
fn missing_values_say_unknown() {
    let d = mono_u8_unknown_length();
    assert_eq!(value(&d, "size_bytes"), "unknown");
    assert_eq!(value(&d, "frames"), "unknown");
    assert_eq!(value(&d, "duration_seconds"), "unknown");
}

#[test]
fn sample_formats_and_codecs_read_in_plain_words_with_the_raw_name_kept() {
    let with = |codec: &str, format: &str| {
        let mut s = stream(0);
        s.codec = codec.into();
        s.sample_format = format.into();
        doc(None, vec![s])
    };
    for (codec, format, friendly_codec, friendly_format) in [
        ("pcm_u8", "u8", "PCM 8-bit unsigned", "8-bit"),
        ("pcm_s8", "u8", "PCM 8-bit signed", "8-bit"),
        ("pcm_s16le", "s16", "PCM 16-bit LE", "16-bit"),
        ("pcm_s24be", "s24", "PCM 24-bit BE", "24-bit"),
        ("pcm_s32le", "s32", "PCM 32-bit LE", "32-bit"),
        ("pcm_f32le", "f32", "PCM 32-bit float LE", "32-bit float"),
        ("pcm_f64be", "f64", "PCM 64-bit float BE", "64-bit float"),
        ("flac", "s16", "FLAC", "16-bit"),
        ("mystery", "weird", "mystery", "weird"),
    ] {
        let d = with(codec, format);
        assert_eq!(value(&d, "codec"), friendly_codec);
        assert_eq!(raw(&d, "codec"), codec);
        assert_eq!(value(&d, "sample_format"), friendly_format);
        assert_eq!(raw(&d, "sample_format"), format);
    }
}

#[test]
fn container_names_read_as_people_write_them() {
    for (raw_name, shown) in [
        ("wav", "WAV"),
        ("flac", "FLAC"),
        ("aiff", "AIFF"),
        ("ogg", "Ogg"),
        ("mp3", "MP3"),
        ("mp4", "MP4"),
        ("adts", "AAC (ADTS)"),
    ] {
        let mut d = mono_u8_unknown_length();
        d.format = raw_name.into();
        assert_eq!(value(&d, "format"), shown);
        assert_eq!(raw(&d, "format"), raw_name);
    }
    assert_eq!(crossfade_app::inspector::format_name("future"), "future");
}

#[test]
fn compressed_codec_names_read_as_people_write_them() {
    use crossfade_app::inspector::codec_name;
    for (raw_name, shown) in [
        ("vorbis", "Vorbis"),
        ("opus", "Opus"),
        ("mp3", "MP3"),
        ("aac", "AAC"),
        ("alac", "ALAC"),
        ("flac", "FLAC"),
        ("future", "future"),
    ] {
        assert_eq!(codec_name(raw_name), shown);
    }
}

#[test]
fn tag_rows_with_the_same_key_keep_separate_identities() {
    let shown = sections(&tagged_20_bit());
    let ids: Vec<&str> = shown[2].rows.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["tags-0", "tags-1", "tags-2", "tags-3"]);
    let mut all: Vec<&str> = shown
        .iter()
        .flat_map(|s| s.rows.iter().map(|r| r.id.as_str()))
        .collect();
    let n = all.len();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), n, "every row id is unique");
    // Non-tag rows are named by their probe key.
    assert_eq!(shown[1].rows[0].id, shown[1].rows[0].field);
}

#[test]
fn the_depth_of_a_stream_is_its_raw_bits_when_it_has_them() {
    let mut s = stream(0);
    s.sample_format = "s24".into();
    assert_eq!(depth(&s).as_deref(), Some("24-bit"));
    s.bits_per_raw_sample = Some(20);
    assert_eq!(depth(&s).as_deref(), Some("20-bit"));
}

/// A 20-bit stereo WAV as other software writes it: `WAVE_FORMAT_EXTENSIBLE`, 24-bit container.
fn extensible_20_bit_wav() -> Vec<u8> {
    let mut fmt = Vec::new();
    for (v, n) in [
        (0xFFFEu32, 2),
        (2, 2),
        (48_000, 4),
        (288_000, 4),
        (6, 2),
        (24, 2),
    ] {
        fmt.extend_from_slice(&v.to_le_bytes()[..n]);
    }
    fmt.extend_from_slice(&22u16.to_le_bytes());
    fmt.extend_from_slice(&20u16.to_le_bytes());
    fmt.extend_from_slice(&3u32.to_le_bytes());
    fmt.extend_from_slice(&1u16.to_le_bytes());
    fmt.extend_from_slice(&[0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71]);
    let pcm = [0u8; 12];
    let mut body = b"WAVEfmt ".to_vec();
    body.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    body.extend_from_slice(&fmt);
    body.extend_from_slice(b"data");
    body.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    body.extend_from_slice(&pcm);
    let mut wav = b"RIFF".to_vec();
    wav.extend_from_slice(&(body.len() as u32).to_le_bytes());
    wav.extend_from_slice(&body);
    wav
}

#[test]
fn a_wav_with_valid_bits_below_its_container_shows_the_true_depth() {
    use crossfade_core::Limits;
    use crossfade_testkit::memory::MemoryIo;
    let mut io = MemoryIo::new();
    io.insert("a.wav", extensible_20_bit_wav());
    let report = crossfade_graph::probe("a.wav", &mut io, &Limits::default()).unwrap();
    let d = report.document();
    assert_eq!(d.streams[0].sample_format, "s24");
    assert_eq!(value(&d, "bits_per_raw_sample"), "20-bit");
    assert_eq!(depth(&d.streams[0]).as_deref(), Some("20-bit"));
}

/// An MP3 stream as its decoder reports it: 32-bit float, which says nothing about the file.
fn lossy_document() -> ProbeDocument {
    use crossfade_core::{AudioSpec, ChannelLayout, CodecId, SampleFormat, StreamInfo, TimeBase};
    let audio = AudioSpec::new(44_100, SampleFormat::F32, ChannelLayout::stereo()).unwrap();
    let mut stream = StreamInfo::new(0, CodecId::Mp3, audio, TimeBase::new(1, 44_100).unwrap());
    stream.frames = Some(44_100);
    crossfade_graph::ProbeReport {
        format: crossfade_format::FormatId::Mp3,
        size_bytes: Some(1000),
        streams: vec![stream],
        tags: Default::default(),
        loudness: None,
    }
    .document()
}

#[test]
fn a_lossy_stream_shows_no_bit_depth() {
    let d = lossy_document();
    assert_eq!(depth(&d.streams[0]), None);
    let shown = render(&sections(&d));
    assert!(!shown.contains("32-bit"), "{shown}");
    assert!(!shown.contains("Bit depth"), "{shown}");
    assert_eq!(value(&d, "sample_format"), "lossy, no bit depth");
    assert_eq!(raw(&d, "sample_format"), "f32");
}

#[test]
fn a_lossless_stream_still_shows_its_depth() {
    let d = tagged_20_bit();
    assert_eq!(depth(&d.streams[0]).as_deref(), Some("20-bit"));
    assert_eq!(
        depth(&stereo_s24_96k().streams[0]).as_deref(),
        Some("24-bit")
    );
}
