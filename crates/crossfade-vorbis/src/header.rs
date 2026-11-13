//! The identification and comment headers (specification sections 4.2.2 and 5).

use crate::error::{Error, Result, malformed};

/// Which of the three header packets a packet is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderKind {
    Ident,
    Comment,
    Setup,
}

/// Which header `packet` is, from its type byte and the "vorbis" signature; `None` for an audio
/// packet (or anything else).
pub fn header_kind(packet: &[u8]) -> Option<HeaderKind> {
    if packet.get(1..7) != Some(b"vorbis") {
        return None;
    }
    match packet[0] {
        1 => Some(HeaderKind::Ident),
        3 => Some(HeaderKind::Comment),
        5 => Some(HeaderKind::Setup),
        _ => None,
    }
}

/// The identification header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ident {
    pub channels: u8,
    pub sample_rate: u32,
    /// Bitrate hints in bits per second; 0 or negative when unset.
    pub bitrate_maximum: i32,
    pub bitrate_nominal: i32,
    pub bitrate_minimum: i32,
    /// The short and the long block size, in samples: powers of two from 64 to 8192.
    pub block_sizes: [u16; 2],
}

impl Ident {
    pub fn parse(packet: &[u8]) -> Result<Self> {
        if packet.len() < 30 {
            return Err(malformed("identification header too short"));
        }
        if header_kind(packet) != Some(HeaderKind::Ident) {
            return Err(malformed("not an identification header"));
        }
        let u32_at = |at: usize| {
            u32::from_le_bytes([packet[at], packet[at + 1], packet[at + 2], packet[at + 3]])
        };
        if u32_at(7) != 0 {
            return Err(malformed(format!("version {} is not Vorbis I", u32_at(7))));
        }
        let channels = packet[11];
        if channels == 0 {
            return Err(malformed("zero channels"));
        }
        let sample_rate = u32_at(12);
        if sample_rate == 0 {
            return Err(malformed("sample rate of zero"));
        }
        let (e0, e1) = (packet[28] & 15, packet[28] >> 4);
        if !(6..=13).contains(&e0) || !(6..=13).contains(&e1) || e0 > e1 {
            return Err(malformed(format!(
                "block sizes 2^{e0} and 2^{e1} (each must be 64 to 8192, short no longer than long)"
            )));
        }
        if packet[29] & 1 == 0 {
            return Err(malformed("identification header framing bit not set"));
        }
        Ok(Self {
            channels,
            sample_rate,
            bitrate_maximum: u32_at(16) as i32,
            bitrate_nominal: u32_at(20) as i32,
            bitrate_minimum: u32_at(24) as i32,
            block_sizes: [1 << e0, 1 << e1],
        })
    }
}

/// The comment header: a vendor string and user comments, conventionally `KEY=value`. Invalid
/// UTF-8 is replaced, not refused.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Comments {
    pub vendor: String,
    pub entries: Vec<String>,
}

impl Comments {
    /// Reads a comment header, refusing one whose strings add up to more than `max_bytes`.
    /// Every length is checked against what the packet holds before anything is allocated. A
    /// missing framing bit is tolerated: tag editors drop it and nothing depends on it.
    pub fn parse(packet: &[u8], max_bytes: usize) -> Result<Self> {
        if header_kind(packet) != Some(HeaderKind::Comment) {
            return Err(malformed("not a comment header"));
        }
        let mut rest = &packet[7..];
        let mut used = 0usize;
        let mut take = |rest: &mut &[u8], what: &str| -> Result<String> {
            let len = take_u32(rest).ok_or_else(|| malformed(format!("{what} length missing")))?;
            let len = usize::try_from(len).unwrap_or(usize::MAX);
            if len > rest.len() {
                return Err(malformed(format!(
                    "{what} of {len} bytes runs past the end of the header"
                )));
            }
            used = used.saturating_add(len);
            if used > max_bytes {
                return Err(Error::OverBudget {
                    requested: used,
                    budget: max_bytes,
                });
            }
            let (s, tail) = rest.split_at(len);
            *rest = tail;
            Ok(String::from_utf8_lossy(s).into_owned())
        };
        let vendor = take(&mut rest, "vendor string")?;
        let count = take_u32(&mut rest).ok_or_else(|| malformed("comment count missing"))?;
        let count = usize::try_from(count).unwrap_or(usize::MAX);
        // Each comment takes at least its four length bytes.
        if count > rest.len() / 4 {
            return Err(malformed(format!(
                "{count} comments cannot fit in the {} bytes left",
                rest.len()
            )));
        }
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            entries.push(take(&mut rest, "comment")?);
        }
        Ok(Self { vendor, entries })
    }

    /// The entries that have a `=`, split at the first one.
    pub fn pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().filter_map(|e| e.split_once('='))
    }
}

fn take_u32(rest: &mut &[u8]) -> Option<u32> {
    let (head, tail) = rest.split_first_chunk::<4>()?;
    *rest = tail;
    Some(u32::from_le_bytes(*head))
}

#[cfg(test)]
#[path = "header_tests.rs"]
mod tests;
