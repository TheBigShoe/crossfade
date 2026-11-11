//! Ogg FLAC writer (RFC 9639 section 10.1): an identification page, a Vorbis comment page,
//! then the FLAC frames as packets.
//!
//! Like the native FLAC writer, the STREAMINFO written first is provisional when the encoder
//! produced it; `set_extradata` brings the final one and `finish` rewrites the first page with
//! it (the page keeps its size, so nothing after it moves). A sink that cannot seek keeps the
//! provisional header, a valid stream with unknown totals. Stream copy hands over the source's
//! STREAMINFO and never calls `set_extradata`, so nothing is patched.
//!
//! The serial number is a constant: the same input gives the same bytes on every run. A file
//! holds one logical stream, so nothing needs the serial to be unique.

use super::writer::{BOS, PageWriter, page_bytes};
use crate::vorbis_comment;
use crossfade_core::{CodecId, Error, MediaSink, Muxer, Packet, SampleFormat, StreamInfo, Tags};
use crossfade_flac::streaminfo::StreamInfoBlock;
use std::io::SeekFrom;

/// "CXFD" read as a big-endian number.
const SERIAL: u32 = 0x4358_4644;
const TYPE_VORBIS_COMMENT: u8 = 4;
const LAST_BLOCK: u8 = 0x80;
/// Block lengths are 24 bits.
const MAX_BLOCK_LEN: usize = (1 << 24) - 1;
/// The comment is the only header packet after the identification one.
const HEADER_PACKETS: u16 = 1;

struct OggMuxer {
    sink: Box<dyn MediaSink>,
    pages: PageWriter,
    /// Samples (per channel) in the packets written so far.
    samples: u64,
    final_header: Option<[u8; StreamInfoBlock::LEN]>,
}

// `flac::mux` keeps its `synthesised` private; this repeats it until the native writer shares it.
fn synthesised(stream: &StreamInfo) -> Result<[u8; StreamInfoBlock::LEN], Error> {
    let audio = &stream.audio;
    let channels = audio.layout().channels();
    if channels > 8 {
        return Err(Error::Unsupported {
            what: format!("flac holds at most 8 channels, got {channels}"),
        });
    }
    if audio.sample_rate() >= 1 << 20 {
        return Err(Error::Unsupported {
            what: format!(
                "flac sample rates stop at 1048575 Hz, got {}",
                audio.sample_rate()
            ),
        });
    }
    let bits = stream.bits_per_raw_sample.unwrap_or(match audio.format() {
        SampleFormat::U8 => 8,
        SampleFormat::S16 => 16,
        SampleFormat::S24 => 24,
        SampleFormat::S32 | SampleFormat::F32 | SampleFormat::F64 => 32,
    });
    Ok(StreamInfoBlock {
        min_block_size: 0,
        max_block_size: 0,
        min_frame_size: 0,
        max_frame_size: 0,
        sample_rate: audio.sample_rate(),
        channels: channels as u8,
        bits_per_sample: bits,
        total_samples: stream.frames.filter(|&n| n < 1 << 36).unwrap_or(0),
        md5: [0; 16],
    }
    .write())
}

fn checked(extradata: &[u8]) -> Result<[u8; StreamInfoBlock::LEN], Error> {
    StreamInfoBlock::parse(extradata).map_err(crate::flac::engine_error)?;
    Ok(extradata.try_into().expect("parse accepted 34 bytes"))
}

fn block_header(flags_and_type: u8, len: usize) -> [u8; 4] {
    let l = (len as u32).to_be_bytes();
    [flags_and_type, l[1], l[2], l[3]]
}

/// The first packet: `0x7F "FLAC"`, version 1.0, header count, `fLaC`, the STREAMINFO block.
fn identification(info: &[u8; StreamInfoBlock::LEN]) -> Vec<u8> {
    let mut p = Vec::with_capacity(51);
    p.extend(b"\x7fFLAC\x01\x00");
    p.extend(HEADER_PACKETS.to_be_bytes());
    p.extend(b"fLaC");
    p.extend(block_header(0, StreamInfoBlock::LEN));
    p.extend(info);
    p
}

pub fn create(
    mut sink: Box<dyn MediaSink>,
    stream: &StreamInfo,
    tags: &Tags,
) -> Result<Box<dyn Muxer>, Error> {
    if stream.codec != CodecId::Flac {
        return Err(Error::Unsupported {
            what: format!("ogg files can only hold flac, not {}", stream.codec.name()),
        });
    }
    let info = if stream.extradata.len() == StreamInfoBlock::LEN {
        checked(&stream.extradata)?
    } else {
        synthesised(stream)?
    };
    let mut comment = Vec::new();
    vorbis_comment::write(
        &format!("Crossfade {}", env!("CARGO_PKG_VERSION")),
        tags,
        &mut comment,
    );
    if comment.len() > MAX_BLOCK_LEN {
        return Err(Error::Unsupported {
            what: "tags are too large for one flac metadata block".into(),
        });
    }
    let mut pages = PageWriter::new(SERIAL);
    pages.packet(&identification(&info), 0);
    pages.end_page();
    let mut packet = block_header(LAST_BLOCK | TYPE_VORBIS_COMMENT, comment.len()).to_vec();
    packet.extend(comment);
    pages.packet(&packet, 0);
    pages.end_page();
    sink.write_all(&pages.take())?;
    Ok(Box::new(OggMuxer {
        sink,
        pages,
        samples: 0,
        final_header: None,
    }))
}

impl Muxer for OggMuxer {
    fn write_packet(&mut self, packet: &Packet) -> Result<(), Error> {
        self.samples = self.samples.saturating_add(packet.duration);
        self.pages.packet(&packet.data, self.samples);
        let ready = self.pages.take();
        if !ready.is_empty() {
            self.sink.write_all(&ready)?;
        }
        Ok(())
    }

    fn set_extradata(&mut self, extradata: &[u8]) -> Result<(), Error> {
        self.final_header = Some(checked(extradata)?);
        Ok(())
    }

    fn finish(&mut self) -> Result<(), Error> {
        self.pages.finish();
        self.sink.write_all(&self.pages.take())?;
        if let Some(header) = self.final_header {
            // A sink that cannot seek keeps the provisional header, which is still valid.
            if self.sink.seek(SeekFrom::Start(0)).is_ok() {
                let packet = identification(&header);
                let lacing = [packet.len() as u8];
                self.sink
                    .write_all(&page_bytes(BOS, 0, SERIAL, 0, &lacing, &packet))?;
                self.sink.seek(SeekFrom::End(0))?;
            }
        }
        self.sink.finish()?;
        Ok(())
    }
}
