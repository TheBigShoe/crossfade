//! Opus packets: the TOC byte and the frame packing of RFC 6716 section 3, every validity rule
//! (R1-R7) enforced, and the self-delimiting variant of Appendix B (each stream but the last
//! of a multistream packet). Ported from libopus `opus_packet_parse_impl`, with RFC 8251's
//! padding fix (section 4): lengths are subtracted from what remains, so no count overflows.
//!
//! Parsing copies nothing: a [`Packet`] holds slices of the input. A malformed packet is an
//! [`Error::InvalidPacket`], never a panic.

use crate::error::{Error, Result};

/// Most frames in a packet: 120 ms of 2.5 ms frames (R5).
pub const MAX_FRAMES: usize = 48;
/// Longest frame (R2): the largest one- or two-byte length.
pub const MAX_FRAME_LEN: usize = 1275;
/// Most audio in a packet, in samples at 48 kHz: 120 ms (R5).
pub const MAX_PACKET_SAMPLES: usize = 5760;

/// The operating mode a packet's configuration selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    /// The linear-prediction layer alone (NB, MB, WB).
    Silk,
    /// SILK up to 8 kHz (at WB) plus CELT above it (SWB, FB).
    Hybrid,
    /// The MDCT layer alone.
    Celt,
}

/// A packet's audio bandwidth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Bandwidth {
    /// Narrowband, 4 kHz (SILK at 8 kHz).
    Narrow,
    /// Medium-band, 6 kHz (SILK at 12 kHz).
    Medium,
    /// Wideband, 8 kHz (SILK at 16 kHz).
    Wide,
    /// Super-wideband, 12 kHz.
    SuperWide,
    /// Fullband, 20 kHz.
    Full,
}

impl Bandwidth {
    /// The CELT band past the last one coded at this bandwidth (`opus_decoder.c`'s `endband`).
    pub fn celt_end_band(self) -> usize {
        match self {
            Self::Narrow => 13,
            Self::Medium | Self::Wide => 17,
            Self::SuperWide => 19,
            Self::Full => 21,
        }
    }
}

/// The table-of-contents byte that starts every packet (RFC 6716 3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Toc(pub u8);

impl Toc {
    /// The configuration number, 0 to 31 (Table 2).
    pub fn config(self) -> u8 {
        self.0 >> 3
    }

    pub fn mode(self) -> Mode {
        match self.config() {
            0..=11 => Mode::Silk,
            12..=15 => Mode::Hybrid,
            _ => Mode::Celt,
        }
    }

    pub fn bandwidth(self) -> Bandwidth {
        match self.config() {
            0..=3 | 16..=19 => Bandwidth::Narrow,
            4..=7 => Bandwidth::Medium,
            8..=11 | 20..=23 => Bandwidth::Wide,
            12..=13 | 24..=27 => Bandwidth::SuperWide,
            _ => Bandwidth::Full,
        }
    }

    /// One frame's duration in samples at 48 kHz: 120, 240, 480, 960, 1920 or 2880.
    pub fn frame_samples(self) -> usize {
        let c = usize::from(self.config());
        match self.mode() {
            Mode::Silk => [480, 960, 1920, 2880][c % 4],
            Mode::Hybrid => [480, 960][c % 2],
            Mode::Celt => [120, 240, 480, 960][c % 4],
        }
    }

    /// Whether the frames are coded in stereo (`s`).
    pub fn stereo(self) -> bool {
        self.0 & 0x04 != 0
    }

    /// The frame count code `c`, 0 to 3.
    pub fn code(self) -> u8 {
        self.0 & 0x03
    }
}

/// A parsed packet: its TOC and its frames, slices of the input.
#[derive(Debug, Clone)]
pub struct Packet<'a> {
    toc: Toc,
    frames: [&'a [u8]; MAX_FRAMES],
    count: usize,
    padding: &'a [u8],
    byte_len: usize,
}

impl<'a> Packet<'a> {
    /// A packet whose length the transport gives (RFC 6716 section 3): all of `data`.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        parse(data, false)
    }

    /// A self-delimited packet (Appendix B) at the start of `data`, as each stream but the
    /// last of a multistream packet is; [`byte_len`](Self::byte_len) says where it ends.
    pub fn parse_self_delimited(data: &'a [u8]) -> Result<Self> {
        parse(data, true)
    }

    pub fn toc(&self) -> Toc {
        self.toc
    }

    /// The frames, 1 to 48, in order; a frame may be empty (DTX or lost).
    pub fn frames(&self) -> &[&'a [u8]] {
        &self.frames[..self.count]
    }

    /// The Opus padding at the end of a code 3 packet (its length bytes excluded).
    pub fn padding(&self) -> &'a [u8] {
        self.padding
    }

    /// The bytes of input the packet takes: all of it unless self-delimited.
    pub fn byte_len(&self) -> usize {
        self.byte_len
    }

    /// The packet's audio in samples at 48 kHz.
    pub fn duration(&self) -> usize {
        self.count * self.toc.frame_samples()
    }
}

fn invalid(what: &'static str) -> Error {
    Error::InvalidPacket(what)
}

/// A one- or two-byte frame length (RFC 6716 3.2.1) at the start of `data`: `(length, bytes)`.
fn parse_size(data: &[u8]) -> Result<(usize, usize)> {
    match *data {
        [] => Err(invalid("a frame length is missing")),
        [b, ..] if b < 252 => Ok((usize::from(b), 1)),
        [_] => Err(invalid("a two-byte frame length is cut short")),
        [b0, b1, ..] => Ok((4 * usize::from(b1) + usize::from(b0), 2)),
    }
}

fn parse(data: &[u8], self_delimited: bool) -> Result<Packet<'_>> {
    let (&toc, _) = data
        .split_first()
        .ok_or(invalid("a packet is empty (R1)"))?;
    let toc = Toc(toc);
    // The headers are read at `pos`; `end` is where the frames stop (padding lies after it).
    let mut pos = 1;
    let mut end = data.len();
    let mut sizes = [0usize; MAX_FRAMES];
    let mut pad = 0;
    let mut cbr = false;
    // What the last frame (or, CBR, each frame) has when no length is coded for it.
    let mut last_size = end - pos;
    let count = match toc.code() {
        0 => 1,
        1 => {
            cbr = true;
            if !self_delimited {
                if !(end - pos).is_multiple_of(2) {
                    return Err(invalid("a code 1 packet has an odd payload (R3)"));
                }
                last_size = (end - pos) / 2;
                sizes[0] = last_size;
            }
            2
        }
        2 => {
            let (size, bytes) = parse_size(&data[pos..end])?;
            pos += bytes;
            if size > end - pos {
                return Err(invalid("a code 2 packet's first frame is too long (R4)"));
            }
            sizes[0] = size;
            last_size = end - pos - size;
            2
        }
        _ => {
            let &ch = data
                .get(pos)
                .ok_or(invalid("a code 3 packet has no frame count (R6)"))?;
            pos += 1;
            let count = usize::from(ch & 0x3F);
            if count == 0 || count * toc.frame_samples() > MAX_PACKET_SAMPLES {
                return Err(invalid(
                    "a code 3 packet holds no frame or over 120 ms (R5)",
                ));
            }
            if ch & 0x40 != 0 {
                // Padding comes off the end as its length is read, so no count can overflow
                // (RFC 8251 section 4).
                loop {
                    if pos >= end {
                        return Err(invalid("the padding length is cut short (R6, R7)"));
                    }
                    let p = data[pos];
                    pos += 1;
                    let add = if p == 255 { 254 } else { usize::from(p) };
                    end = end
                        .checked_sub(add)
                        .filter(|&e| e >= pos)
                        .ok_or(invalid("the padding is longer than the packet (R6, R7)"))?;
                    pad += add;
                    if p != 255 {
                        break;
                    }
                }
            }
            cbr = ch & 0x80 == 0;
            if !cbr {
                last_size = end - pos;
                for size in sizes.iter_mut().take(count - 1) {
                    let (s, bytes) = parse_size(&data[pos..end])?;
                    pos += bytes;
                    if s > end - pos {
                        return Err(invalid("a code 3 frame length is past the end (R7)"));
                    }
                    *size = s;
                    last_size = last_size
                        .checked_sub(bytes + s)
                        .ok_or(invalid("code 3 frame lengths exceed the packet (R7)"))?;
                }
            } else if !self_delimited {
                last_size = (end - pos) / count;
                if last_size * count != end - pos {
                    return Err(invalid("a CBR code 3 packet does not divide evenly (R6)"));
                }
                sizes[..count - 1].fill(last_size);
            }
            count
        }
    };
    if self_delimited {
        let (size, bytes) = parse_size(&data[pos..end])?;
        pos += bytes;
        if size > end - pos {
            return Err(invalid("a self-delimited length is past the end"));
        }
        if cbr {
            if size * count > end - pos {
                return Err(invalid("self-delimited CBR frames are past the end"));
            }
            sizes[..count - 1].fill(size);
        } else if bytes + size > last_size {
            return Err(invalid("a self-delimited last frame is past the end"));
        }
        sizes[count - 1] = size;
    } else {
        if last_size > MAX_FRAME_LEN {
            return Err(invalid("a frame is longer than 1275 bytes (R2)"));
        }
        sizes[count - 1] = last_size;
    }
    let mut frames: [&[u8]; MAX_FRAMES] = [&[]; MAX_FRAMES];
    for (frame, &size) in frames.iter_mut().zip(&sizes[..count]) {
        *frame = &data[pos..pos + size];
        pos += size;
    }
    // Self-delimited, the padding follows the frames; otherwise it ends the packet.
    let padding = &data[pos..pos + pad];
    Ok(Packet {
        toc,
        frames,
        count,
        padding,
        byte_len: pos + pad,
    })
}

/// A packet's duration in samples at 48 kHz from its TOC and frame count alone, without
/// checking the rest (libopus `opus_packet_get_nb_samples`): for timing packets in a
/// container. Errors for an empty packet, a code 3 packet without its count byte or with a
/// count of 0, and more than 120 ms.
pub fn packet_duration(data: &[u8]) -> Result<usize> {
    let toc = Toc(*data.first().ok_or(invalid("a packet is empty (R1)"))?);
    let count = match toc.code() {
        0 => 1,
        1 | 2 => 2,
        _ => usize::from(
            data.get(1)
                .ok_or(invalid("a code 3 packet has no frame count (R6)"))?
                & 0x3F,
        ),
    };
    let samples = count * toc.frame_samples();
    if count == 0 || samples > MAX_PACKET_SAMPLES {
        return Err(invalid("a packet holds no frame or over 120 ms (R5)"));
    }
    Ok(samples)
}

#[cfg(test)]
#[path = "packet_tests.rs"]
mod tests;
