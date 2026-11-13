//! The Ogg Opus header packets of RFC 7845 section 5, as data: no Ogg code lives here. The
//! Ogg reader hands over the first two packets of a stream; the comment fields themselves are
//! parsed by the reader's Vorbis-comment code from [`OpusTags::body`].

use crate::error::{Error, Result};

fn invalid(what: &'static str) -> Error {
    Error::InvalidHeader(what)
}

/// How decoded streams become output channels (RFC 7845 5.1.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelMapping {
    /// 0 (mono or stereo, one stream), 1 (Vorbis order, up to 8 channels), 255 (unidentified)
    /// or a reserved family, read as 255 is (RFC 7845 5.1.1.4). Family 3's demixing matrix
    /// (RFC 8486) is not parsed.
    pub family: u8,
    /// Opus streams per packet, `N` (at least 1).
    pub streams: u8,
    /// How many of the first streams are stereo, `M` (at most `N`).
    pub coupled: u8,
    /// Per output channel, the decoded channel it takes: below `2M` a channel of a coupled
    /// stream (`index / 2`, left if even), below `N + M` mono stream `index - M`, 255 silence.
    /// Family 0's implied table is filled in.
    pub table: Vec<u8>,
}

/// The identification header, `OpusHead` (RFC 7845 5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpusHead {
    /// 0 to 15: any minor version of version 1.
    pub version: u8,
    /// Output channels, `C` (at least 1).
    pub channels: u8,
    /// Samples at 48 kHz to drop from the start of the decoded output.
    pub pre_skip: u16,
    /// The encoder's input rate in Hz, for information only (0: unspecified).
    pub input_sample_rate: u32,
    /// Gain to apply to the output, in dB in Q7.8.
    pub output_gain: i16,
    pub mapping: ChannelMapping,
}

impl OpusHead {
    /// Parses and checks an identification header packet.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let rest = data
            .strip_prefix(b"OpusHead")
            .ok_or(invalid("an identification header starts with OpusHead"))?;
        let [
            version,
            channels,
            p0,
            p1,
            r0,
            r1,
            r2,
            r3,
            g0,
            g1,
            family,
            table @ ..,
        ] = rest
        else {
            return Err(invalid("an identification header is cut short"));
        };
        if version >> 4 != 0 {
            return Err(invalid("an identification header's major version is not 0"));
        }
        let channels = *channels;
        if channels == 0 {
            return Err(invalid("an identification header has no channels"));
        }
        let mapping = match *family {
            0 => {
                if channels > 2 {
                    return Err(invalid("mapping family 0 has one or two channels"));
                }
                ChannelMapping {
                    family: 0,
                    streams: 1,
                    coupled: channels - 1,
                    table: (0..channels).collect(),
                }
            }
            family => {
                if family == 1 && channels > 8 {
                    return Err(invalid("mapping family 1 has at most eight channels"));
                }
                let [streams, coupled, table @ ..] = table else {
                    return Err(invalid("a channel mapping table is cut short"));
                };
                let (streams, coupled) = (*streams, *coupled);
                if streams == 0 {
                    return Err(invalid("a channel mapping has no streams"));
                }
                if coupled > streams {
                    return Err(invalid(
                        "a channel mapping couples more streams than it has",
                    ));
                }
                let decoded = usize::from(streams) + usize::from(coupled);
                if decoded > 255 {
                    return Err(invalid("a channel mapping decodes over 255 channels"));
                }
                let table = table
                    .get(..usize::from(channels))
                    .ok_or(invalid("a channel mapping table is cut short"))?;
                if table.iter().any(|&i| i != 255 && usize::from(i) >= decoded) {
                    return Err(invalid("a channel mapping names a channel not decoded"));
                }
                ChannelMapping {
                    family,
                    streams,
                    coupled,
                    table: table.to_vec(),
                }
            }
        };
        Ok(Self {
            version: *version,
            channels,
            pre_skip: u16::from_le_bytes([*p0, *p1]),
            input_sample_rate: u32::from_le_bytes([*r0, *r1, *r2, *r3]),
            output_gain: i16::from_le_bytes([*g0, *g1]),
            mapping,
        })
    }

    /// The output gain in dB.
    pub fn output_gain_db(&self) -> f64 {
        f64::from(self.output_gain) / 256.0
    }

    /// The factor to scale the output by: `10^(gain / (20 * 256))` (`libm`, so the same bits
    /// everywhere).
    pub fn output_gain_linear(&self) -> f64 {
        libm::pow(10.0, f64::from(self.output_gain) / (20.0 * 256.0))
    }
}

/// The comment header, `OpusTags` (RFC 7845 5.2): its framing checked, its contents as slices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpusTags<'a> {
    /// The vendor string's bytes (UTF-8 by the specification, not checked here).
    pub vendor: &'a [u8],
    pub comment_count: u32,
    /// The Vorbis-comment block (vendor length through the last comment), for a Vorbis-comment
    /// parser.
    pub body: &'a [u8],
    /// Binary data after the comments (preserved by editors when its first bit is 1).
    pub extra: &'a [u8],
}

impl<'a> OpusTags<'a> {
    /// Checks the magic and that every length fits in the packet; allocates nothing.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let body = data
            .strip_prefix(b"OpusTags")
            .ok_or(invalid("a comment header starts with OpusTags"))?;
        let mut rest = body;
        let take_u32 = |rest: &mut &'a [u8]| -> Result<u32> {
            let (n, after) = rest
                .split_first_chunk::<4>()
                .ok_or(invalid("a comment header is cut short"))?;
            *rest = after;
            Ok(u32::from_le_bytes(*n))
        };
        let vendor_len = take_u32(&mut rest)? as usize;
        let vendor = rest
            .get(..vendor_len)
            .ok_or(invalid("a comment header's vendor string is past its end"))?;
        rest = &rest[vendor_len..];
        let comment_count = take_u32(&mut rest)?;
        // Each comment costs at least its 4-byte length.
        if comment_count as usize > rest.len() / 4 {
            return Err(invalid("a comment header claims more comments than fit"));
        }
        for _ in 0..comment_count {
            let len = take_u32(&mut rest)? as usize;
            rest = rest
                .get(len..)
                .ok_or(invalid("a comment is past the end of its header"))?;
        }
        let used = body.len() - rest.len();
        Ok(Self {
            vendor,
            comment_count,
            body: &body[..used],
            extra: rest,
        })
    }
}

#[cfg(test)]
#[path = "head_tests.rs"]
mod tests;
