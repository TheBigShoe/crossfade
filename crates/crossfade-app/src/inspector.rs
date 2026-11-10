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
    pub label: &'static str,
    pub value: String,
}

const UNKNOWN: &str = "unknown";

fn row(field: &'static str, label: &'static str, value: String) -> Row {
    Row {
        field,
        label,
        value,
    }
}

fn or_unknown<T>(v: Option<T>, f: impl FnOnce(T) -> String) -> String {
    v.map_or_else(|| UNKNOWN.to_owned(), f)
}

pub fn sections(doc: &ProbeDocument) -> Vec<Section> {
    let mut out = vec![Section {
        title: "File".into(),
        rows: vec![
            row("format", "Format", doc.format.clone()),
            row("size_bytes", "Size", or_unknown(doc.size_bytes, size)),
        ],
    }];
    out.extend(doc.streams.iter().map(stream_section));
    out
}

fn stream_section(s: &ProbeStream) -> Section {
    Section {
        title: format!("Stream {}", s.index),
        rows: vec![
            row("index", "Index", s.index.to_string()),
            row("type", "Type", s.kind.clone()),
            row("codec", "Codec", s.codec.clone()),
            row("sample_format", "Sample format", s.sample_format.clone()),
            row("sample_rate", "Sample rate", rate(s.sample_rate)),
            row("channels", "Channels", s.channels.to_string()),
            row("layout", "Layout", s.layout.clone()),
            row("frames", "Frames", or_unknown(s.frames, grouped)),
            row(
                "duration_seconds",
                "Duration",
                or_unknown(s.duration_seconds, duration),
            ),
        ],
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
        if i > 0 && (digits.len() - i) % 3 == 0 {
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
