use crossfade_core::{ChannelLayout, Error, JobIo, Limits, StreamInfo, Tags};
use crossfade_format::{FormatId, open_demuxer};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq)]
pub struct ProbeReport {
    pub format: FormatId,
    pub size_bytes: Option<u64>,
    pub streams: Vec<StreamInfo>,
    pub tags: Tags,
}

/// The stable `probe --json` schema (version 1) as plain data, shared by the
/// CLI and every front end so none re-derives the field names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeDocument {
    pub schema: u32,
    pub format: String,
    pub size_bytes: Option<u64>,
    pub streams: Vec<ProbeStream>,
    pub tags: Vec<ProbeTag>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeTag {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeStream {
    pub index: u32,
    #[serde(rename = "type")]
    pub kind: String,
    pub codec: String,
    pub sample_format: String,
    /// Significant bits when fewer than the sample format holds (20 in s24).
    pub bits_per_raw_sample: Option<u8>,
    pub sample_rate: u32,
    pub channels: u16,
    pub layout: String,
    pub frames: Option<u64>,
    pub duration_seconds: Option<f64>,
}

impl ProbeDocument {
    pub const SCHEMA: u32 = 1;
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

impl ProbeReport {
    pub fn document(&self) -> ProbeDocument {
        let streams = self
            .streams
            .iter()
            .map(|s| ProbeStream {
                index: s.index,
                kind: "audio".into(),
                codec: s.codec.name().into(),
                sample_format: s.audio.format().name().into(),
                bits_per_raw_sample: s.bits_per_raw_sample,
                sample_rate: s.audio.sample_rate(),
                channels: s.audio.layout().channels(),
                layout: layout_name(s.audio.layout()),
                frames: s.frames,
                duration_seconds: s
                    .frames
                    .map(|f| f as f64 / f64::from(s.audio.sample_rate())),
            })
            .collect();
        ProbeDocument {
            schema: ProbeDocument::SCHEMA,
            format: self.format.name().into(),
            size_bytes: self.size_bytes,
            streams,
            tags: self
                .tags
                .iter()
                .map(|t| ProbeTag {
                    key: t.key.clone(),
                    value: t.value.clone(),
                })
                .collect(),
        }
    }
}

pub fn probe(path: &str, io: &mut dyn JobIo, limits: &Limits) -> Result<ProbeReport, Error> {
    let source = io.open_source(path)?;
    let size_bytes = source.byte_len();
    let (format, demuxer) = open_demuxer(source, limits)?;
    Ok(ProbeReport {
        format,
        size_bytes,
        streams: demuxer.streams().to_vec(),
        tags: demuxer.tags().clone(),
    })
}
