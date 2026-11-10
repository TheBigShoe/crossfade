//! Rendering of a `ProbeReport`: the stable JSON schema and the human text.

use crossfade_graph::ProbeReport;

pub(crate) fn to_json(report: &ProbeReport) -> String {
    // Plain data with string keys cannot fail to serialise.
    serde_json::to_string_pretty(&report.document()).expect("report serialises")
}

pub(crate) fn to_text(report: &ProbeReport) -> String {
    let doc = report.document();
    let mut out = format!("Input: {}", doc.format);
    if let Some(bytes) = doc.size_bytes {
        out.push_str(&format!(", {bytes} bytes"));
    }
    out.push('\n');
    for s in &doc.streams {
        out.push_str(&format!(
            "  Stream {}: audio, {}, {} Hz, {}, {}",
            s.index, s.codec, s.sample_rate, s.layout, s.sample_format
        ));
        if let (Some(secs), Some(frames)) = (s.duration_seconds, s.frames) {
            out.push_str(&format!(", {secs:.3} s ({frames} frames)"));
        }
        out.push('\n');
    }
    out
}
