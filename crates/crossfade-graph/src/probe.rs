use crate::measure::{Measurements, measure_loudness};
use crate::run::CancelToken;
use crossfade_core::{ChannelLayout, Error, JobIo, Limits, StreamInfo, Tags};
use crossfade_dsp::filters::normalize::to_db;
use crossfade_format::{FormatId, open_demuxer};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq)]
pub struct ProbeReport {
    pub format: FormatId,
    pub size_bytes: Option<u64>,
    pub streams: Vec<StreamInfo>,
    pub tags: Tags,
    /// Only when asked for (`ProbeOptions::loudness`).
    pub loudness: Option<Loudness>,
}

/// What `probe_with` is asked to measure beyond the container's own facts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProbeOptions {
    /// Decode the whole file and measure its loudness and peaks.
    pub loudness: bool,
}

/// The `loudness` object of `probe --json`; each value is `None` (`null`) when the meter has
/// none: silence, or a programme under 400 ms (3 s for the range).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Loudness {
    pub integrated_lufs: Option<f64>,
    pub range_lu: Option<f64>,
    pub true_peak_dbtp: Option<f64>,
    pub sample_peak_dbfs: Option<f64>,
}

impl Loudness {
    fn from_measurements(m: &Measurements) -> Self {
        let db = |linear: f64| (linear > 0.0).then(|| to_db(linear));
        Self {
            integrated_lufs: m.integrated,
            range_lu: m.loudness_range,
            true_peak_dbtp: m.true_peak.and_then(db),
            sample_peak_dbfs: db(m.sample_peak),
        }
    }
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
    /// Omitted unless asked for, so the schema-1 document is unchanged without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Boxed so the document stays small where it sits in a front end's enum.
    pub loudness: Option<Box<Loudness>>,
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
            loudness: self.loudness.map(Box::new),
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
        loudness: None,
    })
}

/// `probe`, plus whatever `options` asks for. The loudness pass decodes the whole file, one
/// stream, and holds 16 bytes per 100 ms of it (see `measure_loudness`).
pub fn probe_with(
    path: &str,
    io: &mut dyn JobIo,
    limits: &Limits,
    options: &ProbeOptions,
) -> Result<ProbeReport, Error> {
    probe_with_cancel(path, io, limits, options, &CancelToken::new())
}

/// [`probe_with`] that a long loudness pass can be stopped from.
pub fn probe_with_cancel(
    path: &str,
    io: &mut dyn JobIo,
    limits: &Limits,
    options: &ProbeOptions,
    cancel: &CancelToken,
) -> Result<ProbeReport, Error> {
    let mut report = probe(path, io, limits)?;
    if options.loudness {
        let m = measure_loudness(path, io, limits, cancel)?;
        report.loudness = Some(Loudness::from_measurements(&m));
    }
    Ok(report)
}
