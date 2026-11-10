//! Rendering of a `ProbeReport`: the stable JSON schema and the human text.

use crossfade_core::{ChannelLayout, StreamInfo};
use crossfade_graph::ProbeReport;
use serde::Serialize;

#[derive(Serialize)]
struct Json<'a> {
    schema: u32,
    format: &'a str,
    size_bytes: Option<u64>,
    streams: Vec<JsonStream>,
}

#[derive(Serialize)]
struct JsonStream {
    index: u32,
    #[serde(rename = "type")]
    kind: &'static str,
    codec: &'static str,
    sample_format: &'static str,
    sample_rate: u32,
    channels: u16,
    layout: String,
    frames: Option<u64>,
    duration_seconds: Option<f64>,
}

fn layout_name(layout: &ChannelLayout) -> String {
    if *layout == ChannelLayout::mono() {
        "mono".into()
    } else if *layout == ChannelLayout::stereo() {
        "stereo".into()
    } else {
        format!("{} channels", layout.channels())
    }
}

fn seconds(s: &StreamInfo) -> Option<f64> {
    s.frames
        .map(|f| f as f64 / f64::from(s.audio.sample_rate()))
}

pub(crate) fn to_json(report: &ProbeReport) -> String {
    let streams = report
        .streams
        .iter()
        .map(|s| JsonStream {
            index: s.index,
            kind: "audio",
            codec: s.codec.name(),
            sample_format: s.audio.format().name(),
            sample_rate: s.audio.sample_rate(),
            channels: s.audio.layout().channels(),
            layout: layout_name(s.audio.layout()),
            frames: s.frames,
            duration_seconds: seconds(s),
        })
        .collect();
    let json = Json {
        schema: 1,
        format: report.format.name(),
        size_bytes: report.size_bytes,
        streams,
    };
    // Plain data with string keys cannot fail to serialise.
    serde_json::to_string_pretty(&json).expect("report serialises")
}

pub(crate) fn to_text(report: &ProbeReport) -> String {
    let mut out = format!("Input: {}", report.format.name());
    if let Some(bytes) = report.size_bytes {
        out.push_str(&format!(", {bytes} bytes"));
    }
    out.push('\n');
    for s in &report.streams {
        out.push_str(&format!(
            "  Stream {}: audio, {}, {} Hz, {}, {}",
            s.index,
            s.codec.name(),
            s.audio.sample_rate(),
            layout_name(s.audio.layout()),
            s.audio.format().name()
        ));
        if let (Some(secs), Some(frames)) = (seconds(s), s.frames) {
            out.push_str(&format!(", {secs:.3} s ({frames} frames)"));
        }
        out.push('\n');
    }
    out
}
