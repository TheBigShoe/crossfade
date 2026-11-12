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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpeg_audio_frame_headers_of_every_version_and_layer_are_mp3() {
        for head in [
            [0xFF, 0xFB, 0x90, 0x00], // MPEG-1 layer III, 128 kbit/s, 44.1 kHz
            [0xFF, 0xFA, 0x90, 0x00], // with a CRC
            [0xFF, 0xF3, 0x84, 0x00], // MPEG-2 layer III
            [0xFF, 0xE3, 0x18, 0x00], // MPEG-2.5 layer III
            [0xFF, 0xFD, 0x14, 0x00], // MPEG-1 layer II
            [0xFF, 0xFF, 0x14, 0x00], // MPEG-1 layer I
            [0xFF, 0xFB, 0x08, 0x00], // free format, 32 kHz
        ] {
            assert_eq!(mpeg_audio(&head), Some(FormatId::Mp3), "{head:02X?}");
        }
    }

    #[test]
    fn reserved_fields_are_not_a_frame_header() {
        for head in [
            [0xFF, 0xEB, 0x90, 0x00], // version 01 is reserved
            [0xFF, 0xFB, 0xF0, 0x00], // bitrate index 1111
            [0xFF, 0xFB, 0x9C, 0x00], // sample-rate index 11
            [0xFE, 0xFB, 0x90, 0x00], // sync cut short
            [0xFF, 0xDB, 0x90, 0x00],
            [0x00, 0xFB, 0x90, 0x00],
        ] {
            assert_eq!(mpeg_audio(&head), None, "{head:02X?}");
        }
    }

    #[test]
    fn layer_bits_00_are_adts_with_its_twelve_bit_sync() {
        for head in [
            [0xFF, 0xF1, 0x50, 0x80], // MPEG-4 AAC LC, 44.1 kHz, no CRC
            [0xFF, 0xF1, 0x4C, 0x80], // 48 kHz: index 3, whose low bits look like "rate 11"
            [0xFF, 0xF9, 0x50, 0x80], // MPEG-2 id
            [0xFF, 0xF0, 0x60, 0x80], // with a CRC, 24 kHz
            [0xFF, 0xF1, 0xF0, 0x80], // profile 3 with index 12: "bitrate 1111" to MPEG eyes
        ] {
            assert_eq!(mpeg_audio(&head), Some(FormatId::Adts), "{head:02X?}");
        }
        for head in [
            [0xFF, 0xE1, 0x50, 0x80], // 11-bit sync only, version 00, layer 00
            [0xFF, 0xF1, 0x74, 0x80], // sampling index 13, reserved
            [0xFF, 0xF1, 0x7C, 0x80], // 15, the escape ADTS cannot carry
        ] {
            assert_eq!(mpeg_audio(&head), None, "{head:02X?}");
        }
    }

    #[test]
    fn a_header_needs_three_bytes() {
        assert_eq!(mpeg_audio(&[]), None);
        assert_eq!(mpeg_audio(&[0xFF]), None);
        assert_eq!(mpeg_audio(&[0xFF, 0xFB]), None);
        assert_eq!(mpeg_audio(&[0xFF, 0xFB, 0x90]), Some(FormatId::Mp3));
    }
}
