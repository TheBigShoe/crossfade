use crossfade_app::inspector::{Section, sections};
use crossfade_graph::{ProbeDocument, ProbeStream};
use std::collections::BTreeSet;

fn stream(index: u32) -> ProbeStream {
    ProbeStream {
        index,
        kind: "audio".into(),
        codec: "pcm_u8".into(),
        sample_format: "u8".into(),
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

fn render(sections: &[Section]) -> String {
    let mut out = String::new();
    for s in sections {
        out.push_str(&format!("[{}]\n", s.title));
        for r in &s.rows {
            out.push_str(&format!("  {} ({}): {}\n", r.label, r.field, r.value));
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
    assert_eq!(fields(&shown[1]), keys(stream_json));
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
