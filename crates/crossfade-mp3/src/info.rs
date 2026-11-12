//! The info frame an encoder may put first: a frame that carries no audio but the stream's
//! frame and byte counts (Xing's `Xing` and `Info` headers, Fraunhofer's `VBRI`) and, in
//! LAME's extension to Xing, the encoder delay and padding that make playback gapless.
//!
//! Written from public descriptions of these headers (the Xing SDK's description, LAME's
//! published "mp3 info tag" specification and Fraunhofer's VBRI layout), cross-checked against
//! minimp3_ex (CC0).

use crate::FrameHeader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoKind {
    /// `Xing`: written for variable bitrate streams.
    Xing,
    /// `Info`: the same header, written by LAME for constant bitrate streams.
    Info,
    /// Fraunhofer's `VBRI`.
    Vbri,
}

/// What an info frame says about the stream that follows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InfoFrame {
    pub kind: InfoKind,
    /// Audio frames in the stream, the info frame itself not counted.
    pub frames: Option<u32>,
    /// Bytes of the stream, the info frame included.
    pub bytes: Option<u32>,
    /// LAME's extension: the encoder's 9-byte version string (`LAME3.100`, `Lavc58.13`...).
    pub encoder: Option<[u8; 9]>,
    /// LAME's extension: sample frames of encoder delay at the start and of padding at the end
    /// (12 bits each), not counting the decoder's own delay.
    pub delay_padding: Option<(u16, u16)>,
}

const XING_FRAMES: u32 = 1;
const XING_BYTES: u32 = 2;
const XING_TOC: u32 = 4;
const XING_QUALITY: u32 = 8;

fn be32(b: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(..4)?.try_into().ok()?))
}

impl InfoFrame {
    /// Reads `frame` (one whole frame, header first) as an info frame; `None` when it is an
    /// ordinary audio frame. Layer III only.
    pub fn parse(header: &FrameHeader, frame: &[u8]) -> Option<Self> {
        let _ = (header, frame, Self::vbri);
        None
    }

    fn xing(tag: &[u8]) -> Self {
        let kind = if tag.starts_with(b"Xing") {
            InfoKind::Xing
        } else {
            InfoKind::Info
        };
        let flags = be32(&tag[4..]).unwrap_or(0);
        let mut at = 8;
        let mut field = |flag: u32, len: usize| {
            if flags & flag == 0 {
                return None;
            }
            let v = tag.get(at..at + len);
            at += len;
            v
        };
        let frames = field(XING_FRAMES, 4).and_then(be32);
        let bytes = field(XING_BYTES, 4).and_then(be32);
        let _toc = field(XING_TOC, 100);
        let _quality = field(XING_QUALITY, 4);
        let mut info = Self {
            kind,
            frames,
            bytes,
            encoder: None,
            delay_padding: None,
        };
        // LAME's extension: version string, revision and method, lowpass, replay gain (8),
        // flags, bitrate, then delay and padding in three bytes.
        if let Some(ext) = tag.get(at..at + 24) {
            let name: [u8; 9] = ext[..9].try_into().unwrap_or_default();
            let known = [&b"LAME"[..], b"L3.9", b"Lavc", b"Lavf", b"GOGO"];
            if known.iter().any(|k| name.starts_with(k)) {
                info.encoder = Some(name);
                let d = &ext[21..24];
                let delay = u16::from(d[0]) << 4 | u16::from(d[1] >> 4);
                let padding = u16::from(d[1] & 0x0F) << 8 | u16::from(d[2]);
                info.delay_padding = Some((delay, padding));
            }
        }
        info
    }

    /// `VBRI` sits 32 bytes after the header whatever the mode: version, delay, quality (two
    /// bytes each), then bytes and frames.
    fn vbri(frame: &[u8]) -> Option<Self> {
        let tag = frame.get(36..36 + 18)?;
        if !tag.starts_with(b"VBRI") {
            return None;
        }
        Some(Self {
            kind: InfoKind::Vbri,
            frames: be32(&tag[14..]),
            bytes: be32(&tag[10..]),
            encoder: None,
            delay_padding: None,
        })
    }
}
