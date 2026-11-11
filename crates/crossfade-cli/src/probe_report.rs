//! Rendering of a `ProbeReport`: the stable JSON schema and the human text.

use crossfade_graph::{Loudness, ProbeReport};

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
        if let Some(bits) = s.bits_per_raw_sample {
            out.push_str(&format!(" ({bits}-bit)"));
        }
        if let (Some(secs), Some(frames)) = (s.duration_seconds, s.frames) {
            out.push_str(&format!(", {secs:.3} s ({frames} frames)"));
        }
        out.push('\n');
    }
    if let Some(l) = &doc.loudness {
        out.push_str(&loudness_line(l));
    }
    if !doc.tags.is_empty() {
        out.push_str("Tags:\n");
        for t in &doc.tags {
            out.push_str(&format!("  {}: {}\n", t.key, t.value));
        }
    }
    out
}

/// One line, one decimal. Without an integrated loudness (silence, under 400 ms) it says so;
/// a range needs 3 s, so a short programme can have the first and not the second.
fn loudness_line(l: &Loudness) -> String {
    let mut parts = Vec::new();
    match l.integrated_lufs {
        Some(v) => parts.push(format!("{v:.1} LUFS integrated")),
        None => parts.push("no loudness (silence or under 400 ms)".into()),
    }
    if let Some(v) = l.range_lu {
        parts.push(format!("{v:.1} LU range"));
    }
    if let Some(v) = l.true_peak_dbtp {
        parts.push(format!("{v:.1} dBTP true peak"));
    }
    format!("Loudness: {}\n", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossfade_core::{
        AudioSpec, ChannelLayout, CodecId, SampleFormat, StreamInfo, Tags, TimeBase,
    };
    use crossfade_format::FormatId;

    fn report(bits: Option<u8>, tags: Tags) -> ProbeReport {
        let audio = AudioSpec::new(44100, SampleFormat::S24, ChannelLayout::stereo()).unwrap();
        let stream = StreamInfo {
            frames: Some(44100),
            bits_per_raw_sample: bits,
            ..StreamInfo::new(0, CodecId::Flac, audio, TimeBase::new(1, 44100).unwrap())
        };
        ProbeReport {
            format: FormatId::Flac,
            size_bytes: Some(1234),
            streams: vec![stream],
            tags,
            loudness: None,
        }
    }

    #[test]
    fn plain_report_has_no_tags_block_and_no_bits_note() {
        assert_eq!(
            to_text(&report(None, Tags::new())),
            "Input: flac, 1234 bytes\n  Stream 0: audio, flac, 44100 Hz, stereo, s24, 1.000 s (44100 frames)\n"
        );
    }

    #[test]
    fn raw_bits_and_tags_are_listed_when_present() {
        let mut tags = Tags::new();
        tags.push("ARTIST", "One");
        tags.push("ARTIST", "Two");
        tags.push("TITLE", "A Title");
        assert_eq!(
            to_text(&report(Some(20), tags)),
            "Input: flac, 1234 bytes\n  Stream 0: audio, flac, 44100 Hz, stereo, s24 (20-bit), 1.000 s (44100 frames)\nTags:\n  ARTIST: One\n  ARTIST: Two\n  TITLE: A Title\n"
        );
    }

    #[test]
    fn json_carries_the_new_fields() {
        let mut tags = Tags::new();
        tags.push("TITLE", "T");
        let json = to_json(&report(Some(20), tags));
        assert!(json.contains("\"bits_per_raw_sample\": 20"), "{json}");
        assert!(json.contains("\"key\": \"TITLE\""), "{json}");
    }

    fn loudness(i: Option<f64>, r: Option<f64>, tp: Option<f64>) -> String {
        loudness_line(&Loudness {
            integrated_lufs: i,
            range_lu: r,
            true_peak_dbtp: tp,
            sample_peak_dbfs: None,
        })
    }

    #[test]
    fn loudness_line_variants() {
        assert_eq!(
            loudness(Some(-23.04), Some(5.2), Some(-1.26)),
            "Loudness: -23.0 LUFS integrated, 5.2 LU range, -1.3 dBTP true peak\n"
        );
        assert_eq!(
            loudness(Some(-20.0), None, Some(-3.0)),
            "Loudness: -20.0 LUFS integrated, -3.0 dBTP true peak\n"
        );
        assert_eq!(
            loudness(None, None, None),
            "Loudness: no loudness (silence or under 400 ms)\n"
        );
        assert_eq!(
            loudness(None, None, Some(-6.0)),
            "Loudness: no loudness (silence or under 400 ms), -6.0 dBTP true peak\n"
        );
    }
}
