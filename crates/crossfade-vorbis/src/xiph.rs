//! The three Vorbis headers as one blob, in Xiph lacing (Matroska's CodecPrivate for Vorbis):
//! the packet count less one (2), the lengths of the first two packets (each a run of 255s and
//! a final byte below 255), then the three packets; the third runs to the end.

use crate::error::{Result, malformed};

/// Packs the identification, comment and setup headers.
pub fn pack_headers(headers: [&[u8]; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        3 + headers
            .iter()
            .map(|h| h.len() / 255 + h.len())
            .sum::<usize>(),
    );
    out.push(2);
    for h in &headers[..2] {
        out.extend(std::iter::repeat_n(255u8, h.len() / 255));
        out.push((h.len() % 255) as u8);
    }
    for h in headers {
        out.extend_from_slice(h);
    }
    out
}

/// The identification, comment and setup headers of a packed blob, borrowed from it; every
/// length is checked against what the blob holds.
pub fn unpack_headers(blob: &[u8]) -> Result<[&[u8]; 3]> {
    let bad = |what: &str| malformed(format!("header blob: {what}"));
    let (&count, mut rest) = blob.split_first().ok_or_else(|| bad("empty"))?;
    if count != 2 {
        return Err(bad("not three packets"));
    }
    let mut lengths = [0usize; 2];
    for len in &mut lengths {
        loop {
            let (&b, tail) = rest
                .split_first()
                .ok_or_else(|| bad("lacing runs past the end"))?;
            rest = tail;
            *len += usize::from(b);
            if b < 255 {
                break;
            }
        }
    }
    if lengths[0].saturating_add(lengths[1]) > rest.len() {
        return Err(bad("packets run past the end"));
    }
    let (first, rest) = rest.split_at(lengths[0]);
    let (second, third) = rest.split_at(lengths[1]);
    Ok([first, second, third])
}

#[cfg(test)]
#[path = "xiph_tests.rs"]
mod tests;
