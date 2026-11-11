//! Ogg container (RFC 3533). Only FLAC in Ogg can be read so far; other codecs are named and
//! refused.

mod crc;
mod flac;
mod packets;
mod page;
mod writer;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod testutil;

use crossfade_core::{Demuxer, Error, Limits, MediaSource, Packet, StreamInfo, Tags};
use crossfade_flac::frame::{self, HeaderError};
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

struct OggFlacDemuxer {
    packets: PacketReader,
    streams: [StreamInfo; 1],
    tags: Tags,
    /// Frames of a fixed-block-size stream are numbered in units of this.
    block_size: u32,
    /// The first audio packet, read while looking for the end of the headers.
    held: Vec<u8>,
    has_held: bool,
    done: bool,
}

fn malformed(what: String) -> Error {
    Error::Malformed { what }
}

pub fn open(source: Box<dyn MediaSource>, limits: &Limits) -> Result<Box<dyn Demuxer>, Error> {
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
    let mut flac: Option<(u32, u32, Vec<u8>)> = None;
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
        if name == "flac" && flac.is_none() {
            let lacing = pages.lacing();
            let single = lacing.last().is_some_and(|&l| l < 255)
                && lacing[..lacing.len() - 1].iter().all(|&l| l == 255);
            if !single {
                return Err(malformed(
                    "ogg flac first page holds more than the identification packet".into(),
                ));
            }
            flac = Some((h.serial, h.sequence, pages.body().to_vec()));
        } else if name != "flac" {
            first_other.get_or_insert(name);
        }
        if !pages.next_page()? {
            loaded = false;
            break;
        }
    }
    let Some((serial, sequence, id)) = flac else {
        return Err(Error::Unsupported {
            what: format!(
                "ogg stream holds {}; only flac in ogg can be read so far",
                first_other.unwrap_or("unknown")
            ),
        });
    };
    let header = flac::parse_first_packet(&id)?;
    let mut packets = PacketReader::new(pages, serial, limits.max_packet_bytes);
    packets.resume(sequence.wrapping_add(1), loaded);

    let mut tags = None;
    let mut held = Vec::new();
    // The declared header count is only a hint: the headers end at the first frame.
    let has_held = loop {
        if !packets.next_packet(&mut held)? {
            break false;
        }
        if flac::starts_frame(&held) {
            break true;
        }
        if tags.is_none() {
            tags = flac::comment_tags(&held, limits)?;
        }
    };
    let stream = crate::flac::stream_info(&header.block, &header.streaminfo)?;
    Ok(Box::new(OggFlacDemuxer {
        packets,
        streams: [stream],
        tags: tags.unwrap_or_default(),
        block_size: u32::from(header.block.min_block_size),
        held,
        has_held,
        done: false,
    }))
}

impl Demuxer for OggFlacDemuxer {
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn tags(&self) -> &Tags {
        &self.tags
    }

    fn read_packet(&mut self, out: &mut Packet) -> Result<bool, Error> {
        if self.done {
            return Ok(false);
        }
        if self.has_held {
            self.has_held = false;
            std::mem::swap(&mut out.data, &mut self.held);
        } else if !self.packets.next_packet(&mut out.data)? {
            self.done = true;
            return Ok(false);
        }
        let h = frame::parse_header(&out.data).map_err(|e| {
            malformed(match e {
                HeaderError::NeedMoreData => "ogg flac frame header is cut short".into(),
                HeaderError::Invalid(why) => format!("ogg flac frame header: {why}"),
            })
        })?;
        out.stream = 0;
        out.pts = i64::try_from(h.first_sample(self.block_size)).map_err(|_| Error::Overflow)?;
        out.duration = u64::from(h.block_size);
        Ok(true)
    }
}
