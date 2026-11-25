//! Box assembly for the writer: every size is checked, so a box that cannot be written fails
//! instead of wrapping.

use super::boxes::FourCc;
use crossfade_core::Error;

/// The size field is 32 bits; the 64-bit form is used only for `mdat`, whose header the muxer
/// writes itself.
pub(super) fn too_big(kind: FourCc) -> Error {
    Error::Unsupported {
        what: format!(
            "mp4: the '{}' box would be larger than 4 GiB",
            String::from_utf8_lossy(&kind)
        ),
    }
}

/// A box of `kind` around the concatenation of `parts`.
pub(super) fn boxed(kind: &FourCc, parts: &[&[u8]]) -> Result<Vec<u8>, Error> {
    let len = parts
        .iter()
        .try_fold(8usize, |n, p| n.checked_add(p.len()))
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| too_big(*kind))?;
    let mut out = Vec::with_capacity(len as usize);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(kind);
    for p in parts {
        out.extend_from_slice(p);
    }
    Ok(out)
}

/// A full box: `boxed` after a version byte and 24 bits of flags.
pub(super) fn full(
    kind: &FourCc,
    version: u8,
    flags: u32,
    parts: &[&[u8]],
) -> Result<Vec<u8>, Error> {
    let head = (u32::from(version) << 24 | flags & 0xFF_FFFF).to_be_bytes();
    let mut all: Vec<&[u8]> = Vec::with_capacity(parts.len() + 1);
    all.push(&head);
    all.extend_from_slice(parts);
    boxed(kind, &all)
}
