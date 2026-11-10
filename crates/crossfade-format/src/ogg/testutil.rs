//! A small page writer for tests: builds Ogg streams from packets, with the page splits
//! the caller chooses, so reader edge cases can be written down directly.

use super::crc::crc32;

pub const BOS: u8 = 0x02;
pub const EOS: u8 = 0x04;
pub const CONTINUED: u8 = 0x01;

/// One page's worth of fields; `lacing` and `body` are written as given.
#[derive(Clone, Debug, Default)]
pub struct RawPage {
    pub flags: u8,
    pub granule: i64,
    pub serial: u32,
    pub sequence: u32,
    pub lacing: Vec<u8>,
    pub body: Vec<u8>,
}

impl RawPage {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = b"OggS".to_vec();
        out.push(0);
        out.push(self.flags);
        out.extend(self.granule.to_le_bytes());
        out.extend(self.serial.to_le_bytes());
        out.extend(self.sequence.to_le_bytes());
        out.extend([0; 4]);
        out.push(self.lacing.len() as u8);
        out.extend(&self.lacing);
        out.extend(&self.body);
        let crc = crc32(&out);
        out[22..26].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

/// Lacing values of one packet: 255s, then a value below 255 (a 0 after an exact multiple).
pub fn lacing_of(len: usize) -> Vec<u8> {
    let mut v = vec![255; len / 255];
    v.push((len % 255) as u8);
    v
}

/// Packets as pages. `segments_per_page` cycles; a packet that does not fit is continued
/// on the next page. The first page is marked BOS and the last EOS. Granules are -1.
pub fn pages(
    serial: u32,
    first_sequence: u32,
    packets: &[Vec<u8>],
    per_page: &[usize],
) -> Vec<RawPage> {
    let mut segs: Vec<(u8, &[u8])> = Vec::new();
    for p in packets {
        let lacing = lacing_of(p.len());
        for (i, &l) in lacing.iter().enumerate() {
            let start = i * 255;
            segs.push((l, &p[start..(start + usize::from(l)).min(p.len())]));
        }
    }
    let mut out = Vec::new();
    let mut at = 0;
    let mut i = 0;
    while at < segs.len() {
        let n = per_page[i % per_page.len()]
            .clamp(1, 255)
            .min(segs.len() - at);
        i += 1;
        let chunk = &segs[at..at + n];
        out.push(RawPage {
            flags: if at > 0 && segs[at - 1].0 == 255 {
                CONTINUED
            } else {
                0
            },
            granule: -1,
            serial,
            sequence: first_sequence + out.len() as u32,
            lacing: chunk.iter().map(|s| s.0).collect(),
            body: chunk.iter().flat_map(|s| s.1.iter().copied()).collect(),
        });
        at += n;
    }
    if let Some(first) = out.first_mut() {
        first.flags |= BOS;
    }
    if let Some(last) = out.last_mut() {
        last.flags |= EOS;
    }
    out
}

pub fn bytes(pages: &[RawPage]) -> Vec<u8> {
    pages.iter().flat_map(RawPage::to_bytes).collect()
}
