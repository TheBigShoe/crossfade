//! The Ogg Opus mapping (RFC 7845). Not built yet.

use super::LogicalStream;
use crossfade_core::{Demuxer, Error, Limits};

pub(crate) fn open(stream: LogicalStream, limits: &Limits) -> Result<Box<dyn Demuxer>, Error> {
    let LogicalStream {
        packets,
        serial,
        id,
    } = stream;
    let _ = (packets, serial, id, limits);
    Err(Error::Unsupported {
        what: "opus in ogg is recognised, but reading it is not built yet".into(),
    })
}
