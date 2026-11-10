//! FLAC reader.
//!
//! The input may be a pipe, so nothing seeks and no file is buffered whole. Frames carry no
//! length, so a frame ends where the next acceptable header begins and the CRC-16 over what
//! lies between them checks out; a bare header-looking byte pattern inside audio data does not
//! pass that test.

use super::stream_info;
use crate::vorbis_comment;
use crossfade_core::{Demuxer, Error, Limits, MediaSource, Packet, StreamInfo, Tags};
use crossfade_flac::crc::crc16_update;
use crossfade_flac::frame::{self, FrameHeader, HeaderError};
use crossfade_flac::streaminfo::StreamInfoBlock;
use std::io::ErrorKind;

/// A frame header is at most 16 bytes: sync, two flag bytes, a 7-byte number, two 2-byte
/// extensions and the CRC-8.
const MAX_HEADER: usize = 16;
const READ_CHUNK: usize = 8192;
/// How far into the audio data a first frame may start.
const MAX_LEADING_JUNK: usize = 64 * 1024;
/// False starts tolerated while resynchronising, so hostile junk full of headers stays cheap.
const MAX_FALSE_STARTS: usize = 16;
/// How much may follow the last frame (an ID3v1 or APE tag) before we stop reading and call
/// the frame too long.
const TRAILING_SLACK: usize = 64 * 1024;
/// Shortest frame after its header: a subframe header byte and the CRC-16.
const MIN_AFTER_HEADER: usize = 3;

struct FlacDemuxer {
    source: Box<dyn MediaSource>,
    /// Bytes taken from the source so far.
    pos: u64,
    streams: [StreamInfo; 1],
    tags: Tags,
    max_packet: usize,
    max_frame_size: usize,
    /// Read-ahead; after the first frame it always starts at a frame header.
    buf: Vec<u8>,
    eof: bool,
    done: bool,
    /// Set by the first frame: blocking strategy and block size of a fixed-size stream.
    first: Option<(bool, u32)>,
}

fn malformed(what: &str) -> Error {
    Error::Malformed {
        what: format!("flac: {what}"),
    }
}

/// Fills `buf` unless the source ends first; returns the bytes read.
fn fill(source: &mut dyn MediaSource, pos: &mut u64, buf: &mut [u8]) -> Result<usize, Error> {
    let mut n = 0;
    while n < buf.len() {
        match source.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k.min(buf.len() - n),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    *pos = pos.saturating_add(n as u64);
    Ok(n)
}

fn fill_exact(source: &mut dyn MediaSource, pos: &mut u64, buf: &mut [u8]) -> Result<(), Error> {
    if fill(source, pos, buf)? < buf.len() {
        return Err(Error::UnexpectedEof { offset: *pos });
    }
    Ok(())
}

/// Skips by reading: the source may be a pipe.
fn skip(source: &mut dyn MediaSource, pos: &mut u64, mut len: u64) -> Result<(), Error> {
    let mut scratch = [0u8; 4096];
    while len > 0 {
        let step = scratch
            .len()
            .min(usize::try_from(len).unwrap_or(usize::MAX));
        fill_exact(source, pos, &mut scratch[..step])?;
        len -= step as u64;
    }
    Ok(())
}

/// Reads `len` bytes into a vector that grows only as bytes actually arrive, so a declared
/// length the input cannot back costs nothing.
fn read_body(source: &mut dyn MediaSource, pos: &mut u64, len: usize) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    while out.len() < len {
        let step = chunk.len().min(len - out.len());
        fill_exact(source, pos, &mut chunk[..step])?;
        out.extend_from_slice(&chunk[..step]);
    }
    Ok(out)
}

/// Skips any ID3v2 tags, then checks the stream marker.
fn skip_to_marker(source: &mut dyn MediaSource, pos: &mut u64) -> Result<(), Error> {
    loop {
        let mut head = [0u8; 4];
        if fill(source, pos, &mut head)? < 4 {
            return Err(Error::UnrecognisedFormat);
        }
        if head == *b"fLaC" {
            return Ok(());
        }
        if head[..3] != *b"ID3" {
            return Err(Error::UnrecognisedFormat);
        }
        // After "ID3" and the major version: revision, flags, syncsafe size.
        let mut rest = [0u8; 6];
        fill_exact(source, pos, &mut rest)?;
        let size = rest[2..]
            .iter()
            .fold(0u64, |acc, &b| acc << 7 | u64::from(b & 0x7F));
        let footer = if rest[1] & 0x10 != 0 { 10 } else { 0 };
        skip(source, pos, size + footer)?;
    }
}

struct Metadata {
    info: StreamInfoBlock,
    raw: [u8; StreamInfoBlock::LEN],
    tags: Tags,
}

fn read_metadata(
    source: &mut dyn MediaSource,
    pos: &mut u64,
    limits: &Limits,
) -> Result<Metadata, Error> {
    let mut first: Option<(StreamInfoBlock, [u8; StreamInfoBlock::LEN])> = None;
    let mut tags = None;
    loop {
        let mut head = [0u8; 4];
        fill_exact(source, pos, &mut head)?;
        let last = head[0] & 0x80 != 0;
        let kind = head[0] & 0x7F;
        let len = u64::from(u32::from_be_bytes([0, head[1], head[2], head[3]]));
        if kind == 127 {
            return Err(malformed("metadata block type 127 is invalid"));
        }
        if first.is_none() {
            if kind != 0 {
                return Err(malformed("STREAMINFO is not the first metadata block"));
            }
            if len != StreamInfoBlock::LEN as u64 {
                return Err(malformed("STREAMINFO block is not 34 bytes"));
            }
            let mut raw = [0u8; StreamInfoBlock::LEN];
            fill_exact(source, pos, &mut raw)?;
            first = Some((
                StreamInfoBlock::parse(&raw).map_err(super::engine_error)?,
                raw,
            ));
        } else if kind == 4 && tags.is_none() {
            // The first comment block counts, however it turns out: the audio plays without it.
            let mut parsed = Tags::new();
            if len <= limits.max_alloc_bytes as u64 {
                let body = read_body(source, pos, len as usize)?;
                if let Ok(vc) = vorbis_comment::parse(&body, limits) {
                    parsed = vc.tags;
                }
            } else {
                skip(source, pos, len)?;
            }
            tags = Some(parsed);
        } else {
            skip(source, pos, len)?;
        }
        if last {
            let (info, raw) = first.ok_or_else(|| malformed("no STREAMINFO block"))?;
            return Ok(Metadata {
                info,
                raw,
                tags: tags.unwrap_or_default(),
            });
        }
    }
}

/// Position of the first frame sync code starting at or after `from` that lies wholly inside
/// `data` (a final lone 0xFF is left for when more bytes arrive).
fn find_sync(data: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 1 < data.len() {
        i += find_ff(&data[i..data.len() - 1])?;
        if data[i + 1] & 0xFE == 0xF8 {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Index of the first 0xFF byte, looking eight bytes at a time: compressed audio holds one only
/// every few hundred bytes, so a byte loop spends most of the reader's time here.
fn find_ff(data: &[u8]) -> Option<usize> {
    const ONES: u64 = 0x0101_0101_0101_0101;
    let (words, rest) = data.as_chunks::<8>();
    for (n, w) in words.iter().enumerate() {
        // The classic has-zero-byte test on the complement.
        let x = !u64::from_le_bytes(*w);
        if x.wrapping_sub(ONES) & !x & (ONES << 7) != 0 {
            let at = w.iter().position(|&b| b == 0xFF)?;
            return Some(n * 8 + at);
        }
    }
    rest.iter()
        .position(|&b| b == 0xFF)
        .map(|at| words.len() * 8 + at)
}

/// Largest frame a header can describe, from the channels and depth the stream could hold:
/// the verbatim size of 33-bit samples plus per-subframe overhead.
fn worst_case_frame(block_size: u32, channels: usize) -> usize {
    let per_channel = (block_size as usize * 33).div_ceil(8) + 1 + 4;
    MAX_HEADER + channels * per_channel + 2
}

impl FlacDemuxer {
    /// Appends up to a chunk from the source; `false` at the end of input.
    fn read_more(&mut self) -> Result<bool, Error> {
        if self.eof {
            return Ok(false);
        }
        let old = self.buf.len();
        self.buf.resize(old + READ_CHUNK, 0);
        let n = loop {
            match self.source.read(&mut self.buf[old..]) {
                Ok(n) => break n,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => {
                    self.buf.truncate(old);
                    return Err(e.into());
                }
            }
        };
        let n = n.min(READ_CHUNK);
        self.buf.truncate(old + n);
        self.pos = self.pos.saturating_add(n as u64);
        if n == 0 {
            self.eof = true;
        }
        Ok(n > 0)
    }

    /// Reads until `n` bytes are buffered; `false` if the input ended first.
    fn ensure(&mut self, n: usize) -> Result<bool, Error> {
        while self.buf.len() < n {
            if !self.read_more()? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn frame_cap(&self, h: &FrameHeader) -> usize {
        let channels = usize::from(h.channels.channels());
        let stream_max = if self.max_frame_size > 0 {
            self.max_frame_size
        } else {
            worst_case_frame(h.block_size, channels)
        };
        self.max_packet.min(stream_max)
    }

    /// Where the frame at the start of `buf` ends, and whether the input ended with it (any
    /// bytes after that point are not audio).
    fn frame_end(&mut self, h: &FrameHeader) -> Result<(usize, bool), Error> {
        let cap = self.frame_cap(h);
        let too_long = || malformed(&format!("frame longer than {cap} bytes"));
        let min_end = h.header_len + MIN_AFTER_HEADER;
        let mut crc = 0u16;
        let mut crc_pos = 0usize;
        let mut from = h.header_len + 1;
        loop {
            match find_sync(&self.buf, from) {
                Some(j) => {
                    if j > cap {
                        // Past the longest legal frame: only trailing data can be here.
                        from = j + 1;
                        if from > cap + TRAILING_SLACK {
                            return Err(too_long());
                        }
                        self.ensure(from + MAX_HEADER)?;
                        continue;
                    }
                    self.ensure(j + MAX_HEADER)?;
                    let next = frame::parse_header(&self.buf[j..]);
                    from = j + 1;
                    let Ok(next) = next else {
                        continue;
                    };
                    if next.variable_block_size != h.variable_block_size || j < min_end {
                        continue;
                    }
                    crc = crc16_update(crc, &self.buf[crc_pos..j]);
                    crc_pos = j;
                    if crc == 0 {
                        return Ok((j, false));
                    }
                }
                None => {
                    from = from.max(self.buf.len().saturating_sub(1));
                    if from > cap + TRAILING_SLACK {
                        return Err(too_long());
                    }
                    if !self.read_more()? {
                        break;
                    }
                }
            }
        }
        // The input has ended: this is the last frame, followed perhaps by a tag or padding.
        // Prefer the last span whose CRC-16 holds, so a chance zero inside the frame cannot cut
        // it short. A frame whose CRC ends in a zero byte also has a zero CRC one byte early, and
        // zero padding keeps the CRC at zero, so the two cannot be told apart: zeros stay with
        // the frame and the decoder, which knows where the subframes end, skips them.
        let mut limit = self.buf.len();
        // An ID3v1 tag is the one trailer with a fixed size and a marker.
        if limit >= 128 && self.buf[limit - 128..limit - 125] == *b"TAG" {
            limit -= 128;
        }
        let limit = limit.min(cap);
        let mut crc = 0u16;
        let mut found = None;
        for (i, &b) in self.buf[..limit].iter().enumerate() {
            crc = crc16_update(crc, &[b]);
            if crc == 0 && i + 1 >= min_end {
                found = Some(i + 1);
            }
        }
        match found {
            Some(end) => Ok((end, true)),
            None if self.buf.len() > cap => Err(too_long()),
            None => Err(Error::UnexpectedEof { offset: self.pos }),
        }
    }

    /// Positions `buf` at the first frame, skipping leading junk, and returns its header and
    /// end. `None` when the stream holds no frames at all.
    fn first_frame(&mut self) -> Result<Option<(FrameHeader, usize, bool)>, Error> {
        let mut skipped = 0usize;
        let mut false_starts = 0usize;
        loop {
            self.ensure(MAX_HEADER)?;
            if self.buf.is_empty() {
                return if skipped == 0 {
                    Ok(None)
                } else {
                    Err(malformed("no frame found after the metadata"))
                };
            }
            match frame::parse_header(&self.buf) {
                Ok(h) => match self.frame_end(&h) {
                    Ok((end, last)) => return Ok(Some((h, end, last))),
                    Err(e) if skipped == 0 => return Err(e),
                    Err(Error::Io { kind, message }) => return Err(Error::Io { kind, message }),
                    Err(_) => {
                        false_starts += 1;
                        if false_starts > MAX_FALSE_STARTS {
                            return Err(malformed("no frame found after the metadata"));
                        }
                    }
                },
                Err(HeaderError::NeedMoreData) if self.eof && skipped == 0 => {
                    return Err(Error::UnexpectedEof { offset: self.pos });
                }
                Err(_) => {}
            }
            // Not a frame start: drop up to the next sync code.
            let step = find_sync(&self.buf, 1)
                .unwrap_or(self.buf.len().max(1) - 1)
                .max(1);
            skipped += step;
            self.buf.drain(..step.min(self.buf.len()));
            if skipped > MAX_LEADING_JUNK {
                return Err(malformed(
                    "no frame found within the first 64 KiB after the metadata",
                ));
            }
        }
    }
}

impl Demuxer for FlacDemuxer {
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
        let (header, end, last) = if self.first.is_none() {
            let Some(found) = self.first_frame()? else {
                self.done = true;
                return Ok(false);
            };
            self.first = Some((found.0.variable_block_size, found.0.block_size));
            found
        } else {
            self.ensure(MAX_HEADER)?;
            let h = frame::parse_header(&self.buf).map_err(|_| malformed("lost frame sync"))?;
            let (end, last) = self.frame_end(&h)?;
            (h, end, last)
        };
        // A fixed-size stream numbers frames, so the first frame's size converts to samples.
        let block = self.first.map_or(0, |(_, b)| b);
        let pts = i64::try_from(header.first_sample(block))
            .map_err(|_| malformed("frame position out of range"))?;
        out.stream = 0;
        out.pts = pts;
        out.duration = u64::from(header.block_size);
        out.data.clear();
        out.data.extend_from_slice(&self.buf[..end]);
        self.buf.drain(..end);
        self.done = last;
        Ok(true)
    }
}

pub fn open(mut source: Box<dyn MediaSource>, limits: &Limits) -> Result<Box<dyn Demuxer>, Error> {
    let mut pos = 0u64;
    skip_to_marker(source.as_mut(), &mut pos)?;
    let meta = read_metadata(source.as_mut(), &mut pos, limits)?;
    let stream = stream_info(&meta.info, &meta.raw)?;
    Ok(Box::new(FlacDemuxer {
        source,
        pos,
        streams: [stream],
        tags: meta.tags,
        max_packet: limits.max_packet_bytes,
        max_frame_size: meta.info.max_frame_size as usize,
        buf: Vec::new(),
        eof: false,
        done: false,
        first: None,
    }))
}

#[cfg(test)]
#[path = "demux_tests.rs"]
mod tests;
