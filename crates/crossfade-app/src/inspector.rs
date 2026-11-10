//! The file inspector's model: a probe document as labelled rows. Each row
//! names the JSON key it shows, so a schema change cannot go unnoticed.

use crossfade_graph::{ProbeDocument, ProbeStream};

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub title: String,
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The `probe --json` key this row shows.
    pub field: &'static str,
    /// Unique within a section for a view's element ids: the key, or `tags-<n>` for the
    /// n-th tag, since a key may repeat.
    pub id: String,
    pub label: String,
    /// What a person reads.
    pub value: String,
    /// The probe's own value, for scripts and tests.
    pub raw: String,
}

const UNKNOWN: &str = "unknown";

fn row(field: &'static str, label: &str, value: String) -> Row {
    friendly_row(field, label, value.clone(), value)
}

fn friendly_row(field: &'static str, label: &str, value: String, raw: String) -> Row {
    Row {
        field,
        id: field.to_owned(),
        label: label.to_owned(),
        value,
        raw,
    }
}

/// `wav` reads `WAV`; a container this build has no name for shows as probed.
pub fn format_name(raw: &str) -> String {
    match raw {
        "wav" => "WAV".into(),
        "flac" => "FLAC".into(),
        "aiff" => "AIFF".into(),
        "ogg" => "Ogg".into(),
        other => other.into(),
    }
}

/// `s16` reads `16-bit`, `f32` reads `32-bit float`.
pub fn sample_format_name(raw: &str) -> String {
    match raw {
        "u8" => "8-bit".into(),
        "s16" => "16-bit".into(),
        "s24" => "24-bit".into(),
        "s32" => "32-bit".into(),
        "f32" => "32-bit float".into(),
        "f64" => "64-bit float".into(),
        other => other.into(),
    }
}

/// `pcm_s16le` reads `PCM 16-bit LE`; short enough for a value column.
pub fn codec_name(raw: &str) -> String {
    if raw == "flac" {
        return "FLAC".into();
    }
    let Some(rest) = raw.strip_prefix("pcm_") else {
        return raw.into();
    };
    match rest {
        "u8" => return "PCM 8-bit unsigned".into(),
        "s8" => return "PCM 8-bit signed".into(),
        _ => {}
    }
    let (sample, endian) = if let Some(sample) = rest.strip_suffix("le") {
        (sample, "LE")
    } else if let Some(sample) = rest.strip_suffix("be") {
        (sample, "BE")
    } else {
        return raw.into();
    };
    format!("PCM {} {endian}", sample_format_name(sample))
}

/// The significant bits of a stream: the raw count when it is below the sample format's.
pub fn depth(stream: &ProbeStream) -> String {
    stream.bits_per_raw_sample.map_or_else(
        || sample_format_name(&stream.sample_format),
        |bits| format!("{bits}-bit"),
    )
}

fn or_unknown<T>(v: Option<T>, f: impl FnOnce(T) -> String) -> String {
    v.map_or_else(|| UNKNOWN.to_owned(), f)
}

pub fn sections(doc: &ProbeDocument) -> Vec<Section> {
    let mut out = vec![Section {
        title: "File".into(),
        rows: vec![
            friendly_row(
                "format",
                "Format",
                format_name(&doc.format),
                doc.format.clone(),
            ),
            row("size_bytes", "Size", or_unknown(doc.size_bytes, size)),
        ],
    }];
    out.extend(doc.streams.iter().map(stream_section));
    if !doc.tags.is_empty() {
        out.push(Section {
            title: "Tags".into(),
            rows: doc
                .tags
                .iter()
                .enumerate()
                .map(|(n, t)| Row {
                    id: format!("tags-{n}"),
                    ..row("tags", &title_case(&t.key), t.value.clone())
                })
                .collect(),
        });
    }
    out
}

/// Keys are ASCII by convention. The common run-together ones get their words
/// back; anything else is shown in title case.
fn title_case(key: &str) -> String {
    const WORDS: &[(&str, &str)] = &[
        ("TRACKNUMBER", "Track number"),
        ("TRACKTOTAL", "Track total"),
        ("DISCNUMBER", "Disc number"),
        ("DISCTOTAL", "Disc total"),
        ("ALBUMARTIST", "Album artist"),
    ];
    if let Some((_, label)) = WORDS.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
        return (*label).to_owned();
    }
    let mut chars = key.chars();
    chars.next().map_or_else(String::new, |first| {
        first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect()
    })
}

fn stream_section(s: &ProbeStream) -> Section {
    let mut rows = vec![
        row("index", "Index", s.index.to_string()),
        row("type", "Type", s.kind.clone()),
        friendly_row("codec", "Codec", codec_name(&s.codec), s.codec.clone()),
        friendly_row(
            "sample_format",
            "Sample format",
            sample_format_name(&s.sample_format),
            s.sample_format.clone(),
        ),
        row("sample_rate", "Sample rate", rate(s.sample_rate)),
        row("channels", "Channels", s.channels.to_string()),
        row("layout", "Layout", s.layout.clone()),
        row("frames", "Frames", or_unknown(s.frames, grouped)),
        row(
            "duration_seconds",
            "Duration",
            or_unknown(s.duration_seconds, duration),
        ),
    ];
    if let Some(bits) = s.bits_per_raw_sample {
        // Right after the sample format it qualifies.
        rows.insert(
            4,
            row("bits_per_raw_sample", "Bit depth", format!("{bits}-bit")),
        );
    }
    Section {
        title: format!("Stream {}", s.index),
        rows,
    }
}

fn size(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{bytes} byte{}", if bytes == 1 { "" } else { "s" });
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    // Promote before printing so 999.95 kB reads 1.0 MB, not 1000.0 kB.
    while format!("{value:.1}")
        .parse::<f64>()
        .is_ok_and(|v| v >= 1000.0)
        && unit < 2
    {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", ["kB", "MB", "GB"][unit])
}

fn rate(hz: u32) -> String {
    if hz < 1000 {
        return format!("{hz} Hz");
    }
    let frac = format!("{:03}", hz % 1000);
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        format!("{} kHz", hz / 1000)
    } else {
        format!("{}.{frac} kHz", hz / 1000)
    }
}

fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn duration(seconds: f64) -> String {
    let ms = (seconds.max(0.0) * 1000.0).round() as u64;
    let (total_s, ms) = (ms / 1000, ms % 1000);
    let (h, m, s) = (total_s / 3600, total_s / 60 % 60, total_s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{ms:03}")
    } else {
        format!("{m}:{s:02}.{ms:03}")
    }
}
