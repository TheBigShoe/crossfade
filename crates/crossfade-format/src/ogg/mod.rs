//! Ogg container (RFC 3533). The first logical stream whose codec has a mapping (FLAC, Vorbis,
//! Opus; one file each) is read; other streams are skipped, and a file with none of them is
//! refused by the name of its first stream's codec.

mod crc;
mod flac;
mod mux;
mod opus;
mod packets;
mod page;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod testutil;
mod vorbis;
mod writer;

use crossfade_core::{Demuxer, Error, Limits, MediaSource};
use packets::PacketReader;
use page::PageReader;

/// What the first packet of a logical stream says it is.
fn identify(first_packet: &[u8]) -> &'static str {
    const KNOWN: [(&[u8], &str); 6] = [
        (b"\x7fFLAC", "flac"),
        (b"\x01vorbis", "vorbis"),
        (b"OpusHead", "opus"),
        (b"\x80theora", "theora"),
        (b"Speex   ", "speex"),
        (b"fishead\0", "skeleton"),
    ];
    KNOWN
        .iter()
        .find(|(magic, _)| first_packet.starts_with(magic))
        .map_or("unknown", |&(_, name)| name)
}

/// The codecs that can be read out of Ogg, each in its own file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mapping {
    Flac,
    Vorbis,
    Opus,
}

impl Mapping {
    fn of(name: &str) -> Option<Self> {
        match name {
            "flac" => Some(Self::Flac),
            "vorbis" => Some(Self::Vorbis),
            "opus" => Some(Self::Opus),
            _ => None,
        }
    }
}

/// The logical stream `open` chose, as it hands it to the stream's mapping.
pub(crate) struct LogicalStream {
    /// The stream's packets after the identification packet: positioned past every
    /// beginning-of-stream page, and knowing the stream's serial number and the sequence number
    /// its next page must carry.
    pub(crate) packets: PacketReader,
    /// The identification packet: the whole body of the stream's first page.
    pub(crate) id: Vec<u8>,
}

fn malformed(what: String) -> Error {
    Error::Malformed { what }
}

pub use mux::create;

pub fn open(source: Box<dyn MediaSource>, limits: &Limits) -> Result<Box<dyn Demuxer>, Error> {
    let (mapping, stream) = choose(source, limits)?;
    match mapping {
        Mapping::Flac => flac::open(stream, limits),
        Mapping::Vorbis => vorbis::open(stream, limits),
        Mapping::Opus => opus::open(stream, limits),
    }
}

/// Reads the beginning-of-stream pages and picks the first stream, in page order, that has a
/// mapping.
fn choose(
    source: Box<dyn MediaSource>,
    limits: &Limits,
) -> Result<(Mapping, LogicalStream), Error> {
    let mut pages = PageReader::new(source);
    if !pages.next_page()? {
        return Err(Error::UnexpectedEof { offset: 0 });
    }
    if !pages.header().is_first() {
        return Err(malformed(format!(
            "ogg page at byte {} is not a beginning-of-stream page",
            pages.header().offset
        )));
    }
    // Every logical stream announces itself on a beginning-of-stream page before any other
    // page appears (RFC 3533 section 4).
    let mut chosen: Option<(Mapping, u32, u32, Vec<u8>)> = None;
    let mut first_other: Option<&'static str> = None;
    let mut seen: Vec<u32> = Vec::new();
    let mut loaded = true;
    while pages.header().is_first() {
        let h = *pages.header();
        if seen.contains(&h.serial) {
            return Err(malformed("two ogg streams share a serial number".into()));
        }
        seen.push(h.serial);
        let name = identify(pages.body());
        match Mapping::of(name) {
            Some(mapping) if chosen.is_none() => {
                let lacing = pages.lacing();
                let single = lacing.last().is_some_and(|&l| l < 255)
                    && lacing[..lacing.len() - 1].iter().all(|&l| l == 255);
                if !single {
                    return Err(malformed(format!(
                        "ogg {name} first page holds more than the identification packet"
                    )));
                }
                chosen = Some((mapping, h.serial, h.sequence, pages.body().to_vec()));
            }
            Some(_) => {}
            None => {
                first_other.get_or_insert(name);
            }
        }
        if !pages.next_page()? {
            loaded = false;
            break;
        }
    }
    let Some((mapping, serial, sequence, id)) = chosen else {
        return Err(Error::Unsupported {
            what: format!(
                "ogg stream holds {}; only flac, vorbis and opus in ogg can be read",
                first_other.unwrap_or("unknown")
            ),
        });
    };
    let mut packets = PacketReader::new(pages, serial, limits.max_packet_bytes);
    packets.resume(sequence.wrapping_add(1), loaded);
    Ok((mapping, LogicalStream { packets, id }))
}
