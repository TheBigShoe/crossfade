//! Ogg Opus files (RFC 7845) made in code, for readers' and front ends' tests: the header
//! packets, packets of a chosen TOC from seeded bytes (SILK decodes any bytes to deterministic
//! audio, CELT nearly any), multistream packets, and the pages with their granules.

use crate::testing::packet_writer::self_delimited;

/// An `OpusHead` packet. `mapping` is `(family, streams, coupled, table)`; `None` is family 0.
pub fn opus_head(
    channels: u8,
    pre_skip: u16,
    output_gain: i16,
    mapping: Option<(u8, u8, u8, &[u8])>,
) -> Vec<u8> {
    let mut h = b"OpusHead".to_vec();
    h.push(1);
    h.push(channels);
    h.extend(pre_skip.to_le_bytes());
    h.extend(44_100u32.to_le_bytes());
    h.extend(output_gain.to_le_bytes());
    match mapping {
        None => h.push(0),
        Some((family, streams, coupled, table)) => {
            h.extend([family, streams, coupled]);
            h.extend_from_slice(table);
        }
    }
    h
}

/// An `OpusTags` packet with `NAME=value` entries.
pub fn opus_tags(vendor: &str, entries: &[&str]) -> Vec<u8> {
    let mut t = b"OpusTags".to_vec();
    t.extend((vendor.len() as u32).to_le_bytes());
    t.extend(vendor.as_bytes());
    t.extend((entries.len() as u32).to_le_bytes());
    for e in entries {
        t.extend((e.len() as u32).to_le_bytes());
        t.extend(e.as_bytes());
    }
    t
}

/// `len` bytes from `seed` (xorshift).
pub fn seeded_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 56) as u8
        })
        .collect()
}

/// `count` code 0 packets of TOC `config` (`stereo` coded), each `len` bytes of seeded data
/// after the TOC.
pub fn packets(config: u8, stereo: bool, count: usize, len: usize, seed: u64) -> Vec<Vec<u8>> {
    (0..count)
        .map(|i| {
            let mut p = vec![(config << 3) | (u8::from(stereo) << 2)];
            p.extend(seeded_bytes(seed.wrapping_add(i as u64), len));
            p
        })
        .collect()
}

/// A multistream packet: every stream's packet but the last self-delimited.
pub fn multistream_packet(streams: &[Vec<u8>]) -> Vec<u8> {
    let (last, rest) = streams.split_last().expect("at least one stream");
    let mut out: Vec<u8> = rest.iter().flat_map(|p| self_delimited(p)).collect();
    out.extend_from_slice(last);
    out
}

/// How [`ogg_file`] lays a stream out in Ogg pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OggLayout {
    pub serial: u32,
    /// Audio packets per page, cycled.
    pub packets_per_page: Vec<usize>,
    /// The granule position before the first packet: positive for a stream cut out of a
    /// longer one (RFC 7845 4.5).
    pub start: i64,
    /// Samples the last page's granule leaves off the end (end trimming, RFC 7845 4.4).
    pub end_trim: i64,
}

impl Default for OggLayout {
    fn default() -> Self {
        Self {
            serial: 0x4F50_5553,
            packets_per_page: vec![5],
            start: 0,
            end_trim: 0,
        }
    }
}

/// An Ogg Opus file: `OpusHead` alone on the first page, `OpusTags` on the second, then the
/// audio packets with granules counting 48 kHz samples from `layout.start` (pre-skip
/// included, as RFC 7845 section 4 has them). Every page carries its CRC.
pub fn ogg_file(head: &[u8], tags: &[u8], audio: &[Vec<u8>], layout: &OggLayout) -> Vec<u8> {
    let mut out = Vec::new();
    let mut sequence = 0u32;
    let mut page = |packets: &[&[u8]], granule: i64, flags: u8, out: &mut Vec<u8>| {
        let mut lacing = Vec::new();
        let mut body = Vec::new();
        for p in packets {
            lacing.extend(std::iter::repeat_n(255u8, p.len() / 255));
            lacing.push((p.len() % 255) as u8);
            body.extend_from_slice(p);
        }
        assert!(lacing.len() <= 255, "too many segments for one page");
        let start = out.len();
        out.extend_from_slice(b"OggS\0");
        out.push(flags);
        out.extend_from_slice(&granule.to_le_bytes());
        out.extend_from_slice(&layout.serial.to_le_bytes());
        out.extend_from_slice(&sequence.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.push(lacing.len() as u8);
        out.extend_from_slice(&lacing);
        out.extend_from_slice(&body);
        let crc = ogg_crc(&out[start..]);
        out[start + 22..start + 26].copy_from_slice(&crc.to_le_bytes());
        sequence += 1;
    };
    page(&[head], 0, 0x02, &mut out);
    page(
        &[tags],
        0,
        if audio.is_empty() { 0x04 } else { 0 },
        &mut out,
    );
    let mut pos = layout.start;
    let mut at = 0;
    let mut k = 0;
    while at < audio.len() {
        let n =
            layout.packets_per_page[k % layout.packets_per_page.len()].clamp(1, audio.len() - at);
        k += 1;
        pos += audio[at..at + n]
            .iter()
            .map(|p| crate::packet_duration(p).unwrap_or(0) as i64)
            .sum::<i64>();
        let last = at + n == audio.len();
        let granule = if last { pos - layout.end_trim } else { pos };
        let refs: Vec<&[u8]> = audio[at..at + n].iter().map(Vec::as_slice).collect();
        page(&refs, granule, if last { 0x04 } else { 0 }, &mut out);
        at += n;
    }
    out
}

/// Ogg's CRC-32 (polynomial 0x04C11DB7, no reflection, no final complement).
pub fn ogg_crc(data: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &b in data {
        crc ^= u32::from(b) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OpusHead, OpusTags};

    #[test]
    fn headers_parse_back() {
        let h = OpusHead::parse(&opus_head(3, 312, -256, Some((1, 2, 1, &[0, 2, 1])))).unwrap();
        assert_eq!((h.channels, h.pre_skip, h.output_gain), (3, 312, -256));
        assert_eq!(h.mapping.table, vec![0, 2, 1]);
        let t = opus_tags("v", &["TITLE=x", "R128_TRACK_GAIN=-512"]);
        assert_eq!(OpusTags::parse(&t).unwrap().comment_count, 2);
    }
}
