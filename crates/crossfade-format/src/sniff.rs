//! Which container a byte stream holds, from its first bytes.

use crate::FormatId;

/// The longest magic: `FORM` + size + `AIFF`.
pub(crate) const SNIFF_LEN: usize = 12;

pub(crate) fn identify(head: &[u8]) -> Option<FormatId> {
    match head {
        [b'R', b'I', b'F', b'F', ..] | [b'R', b'F', b'6', b'4', ..] => Some(FormatId::Wav),
        // An ID3v2 tag may precede the stream marker; the FLAC reader skips it.
        [b'f', b'L', b'a', b'C', ..] | [b'I', b'D', b'3', ..] => Some(FormatId::Flac),
        [
            b'F',
            b'O',
            b'R',
            b'M',
            _,
            _,
            _,
            _,
            b'A',
            b'I',
            b'F',
            b'F' | b'C',
            ..,
        ] => Some(FormatId::Aiff),
        [b'O', b'g', b'g', b'S', ..] => Some(FormatId::Ogg),
        _ => None,
    }
}
