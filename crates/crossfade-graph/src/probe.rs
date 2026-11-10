use crossfade_core::{Error, JobIo, Limits, StreamInfo};
use crossfade_format::{FormatId, open_demuxer};

#[derive(Debug, Clone, PartialEq)]
pub struct ProbeReport {
    pub format: FormatId,
    pub size_bytes: Option<u64>,
    pub streams: Vec<StreamInfo>,
}

pub fn probe(path: &str, io: &mut dyn JobIo, limits: &Limits) -> Result<ProbeReport, Error> {
    let source = io.open_source(path)?;
    let size_bytes = source.byte_len();
    let (format, demuxer) = open_demuxer(source, limits)?;
    Ok(ProbeReport {
        format,
        size_bytes,
        streams: demuxer.streams().to_vec(),
    })
}
