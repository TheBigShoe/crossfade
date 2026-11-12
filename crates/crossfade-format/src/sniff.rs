//! Which container a byte stream holds, from its first bytes, read without seeking so a pipe
//! works: the reader gets the bytes back in front of the rest.

use crate::FormatId;
use crate::replay::{Replay, read_up_to};
use crossfade_core::{Error, Limits, MediaSource};

/// What `sniff` found.
pub(crate) struct Sniffed {
    pub(crate) format: FormatId,
    /// The ID3v2 tags in front of an MPEG or ADTS stream, back to back as read; `source` starts
    /// after them. In front of FLAC they stay in `source`, whose reader skips them.
    pub(crate) id3: Option<Vec<u8>>,
    pub(crate) source: Replay,
}

/// Identifies the container. ID3v2 tags (one or more, each checked against the allocation
/// limit before it is read) may precede FLAC or MPEG audio, or ADTS.
pub(crate) fn sniff(mut inner: Box<dyn MediaSource>, limits: &Limits) -> Result<Sniffed, Error> {
    let mut head = Vec::new();
    read_up_to(&mut *inner, &mut head, SNIFF_LEN)?;
    let mut tags = 0;
    while head[tags..].starts_with(b"ID3") {
        read_up_to(&mut *inner, &mut head, tags + ID3_HEADER)?;
        let header = head
            .get(tags..tags + ID3_HEADER)
            .ok_or(Error::UnexpectedEof {
                offset: head.len() as u64,
            })?;
        let end = usize::try_from(tags as u64 + id3v2_len(header)).map_err(|_| Error::Overflow)?;
        limits.check_alloc(end)?;
        read_up_to(&mut *inner, &mut head, end + SNIFF_LEN)?;
        if head.len() < end {
            return Err(Error::UnexpectedEof {
                offset: head.len() as u64,
            });
        }
        tags = end;
    }
    if tags == 0 {
        let format = match identify(&head) {
            Some(f) => f,
            None => {
                read_up_to(&mut *inner, &mut head, JUNK_WINDOW)?;
                mpeg_after_junk(&head).ok_or(Error::UnrecognisedFormat)?
            }
        };
        return Ok(Sniffed {
            format,
            id3: None,
            source: Replay::new(head, inner, 0),
        });
    }
    let format = match &head[tags..] {
        [b'f', b'L', b'a', b'C', ..] => FormatId::Flac,
        after => match mpeg_audio(after) {
            Some(f) => f,
            None => {
                read_up_to(&mut *inner, &mut head, tags + JUNK_WINDOW)?;
                mpeg_after_junk(&head[tags..]).ok_or(Error::UnrecognisedFormat)?
            }
        },
    };
    if format == FormatId::Flac {
        return Ok(Sniffed {
            format,
            id3: None,
            source: Replay::new(head, inner, 0),
        });
    }
    let rest = head.split_off(tags);
    Ok(Sniffed {
        format,
        id3: Some(head),
        source: Replay::new(rest, inner, tags as u64),
    })
}

const ID3_HEADER: usize = 10;

/// A whole ID3v2 tag's length from its ten-byte header: the header, the syncsafe size, and ten
/// more when the footer flag is set.
fn id3v2_len(header: &[u8]) -> u64 {
    let size = header[6..10]
        .iter()
        .fold(0u64, |acc, &b| acc << 7 | u64::from(b & 0x7F));
    let footer = if header[5] & 0x10 != 0 { 10 } else { 0 };
    ID3_HEADER as u64 + size + footer
}

/// The longest magic: `FORM` + size + `AIFF`.
pub(crate) const SNIFF_LEN: usize = 12;

/// How far into the input (after any ID3v2 tags) MPEG audio may start and still be found,
/// past junk or zero padding. Only an input no magic identifies is read this far; the MP3
/// reader uses the same bound.
pub(crate) const MAX_MPEG_JUNK: usize = 64 << 10;

/// Room for a frame candidate at the end of the junk window, and the header confirming it.
const JUNK_WINDOW: usize = MAX_MPEG_JUNK + crossfade_mp3::MAX_FRAME_LEN + 4;

/// MPEG audio starting within the first `MAX_MPEG_JUNK` bytes of `head`: a Layer I-III frame
/// header with another of the same stream where its frame ends. One header alone is not enough
/// (it is two bytes of chance in any binary); free-format frames, whose length the header does
/// not give, are not looked for.
fn mpeg_after_junk(head: &[u8]) -> Option<FormatId> {
    let last = head.len().checked_sub(4)?.min(MAX_MPEG_JUNK);
    (0..=last)
        .filter(|&k| head[k] == 0xFF)
        .any(|k| confirmed_frame(&head[k..]))
        .then_some(FormatId::Mp3)
}

fn confirmed_frame(b: &[u8]) -> bool {
    let header = |b: &[u8]| {
        b.first_chunk::<4>()
            .and_then(|h| crossfade_mp3::FrameHeader::parse(h).ok())
    };
    let Some(h) = header(b) else {
        return false;
    };
    let Some(len) = h.frame_len() else {
        return false;
    };
    b.get(len..)
        .and_then(header)
        .is_some_and(|n| h.same_stream(&n))
}

/// The container whose magic `head` starts with (ID3v2 tags are `sniff`'s).
fn identify(head: &[u8]) -> Option<FormatId> {
    match head {
        [b'R', b'I', b'F', b'F', ..] | [b'R', b'F', b'6', b'4', ..] => Some(FormatId::Wav),
        [b'f', b'L', b'a', b'C', ..] => Some(FormatId::Flac),
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
        [_, _, _, _, b'f', b't', b'y', b'p', ..] => Some(FormatId::Mp4),
        _ => mpeg_audio(head),
    }
}

/// What an MPEG audio frame header at the start of `head` says the stream is: 11 sync bits,
/// then layer bits 00 for AAC in ADTS (whose sync is 12 bits, `0xFFF`, whose sampling index is
/// below 13, and whose profile is not MPEG-2's reserved 3), else MPEG layers I-III (`Mp3`) when the version is not the reserved 01,
/// the bitrate index is not 1111 and the sample-rate index is not 11. The other fields of an
/// ADTS header sit where MPEG's version, bitrate and rate are, so those checks are MPEG's only.
pub(crate) fn mpeg_audio(head: &[u8]) -> Option<FormatId> {
    let [0xFF, b1, b2, ..] = *head else {
        return None;
    };
    if b1 & 0xE0 != 0xE0 {
        return None;
    }
    if (b1 >> 1) & 3 == 0 {
        // MPEG-2 (id bit set) has no profile 3; MPEG-4's is LTP.
        let reserved_profile = b1 & 0x08 != 0 && b2 >> 6 == 3;
        let adts = b1 & 0xF0 == 0xF0 && (b2 >> 2) & 0xF < 13 && !reserved_profile;
        return adts.then_some(FormatId::Adts);
    }
    let (version, bitrate, rate) = ((b1 >> 3) & 3, b2 >> 4, (b2 >> 2) & 3);
    (version != 1 && bitrate != 0xF && rate != 3).then_some(FormatId::Mp3)
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
            // MPEG-2 profile 3 is reserved. A FLAC stream that starts at a frame header
            // (sync 0xFFF8, block-size code 12) reads like this (conformance uncommon/10).
            [0xFF, 0xF8, 0xC9, 0x08],
            [0xFF, 0xF9, 0xC0, 0x80],
        ] {
            assert_eq!(mpeg_audio(&head), None, "{head:02X?}");
        }
    }

    use crate::replay::read_up_to;
    use crossfade_core::{Error, Limits, MediaSource};
    use crossfade_testkit::io::FaultySource;
    use std::io::Cursor;

    const MP3: [u8; 4] = [0xFF, 0xFB, 0x90, 0x00];
    const ADTS: [u8; 4] = [0xFF, 0xF1, 0x50, 0x80];

    /// An ID3v2.4 tag whose body (`size` bytes) looks like an MPEG frame and a FLAC marker, so
    /// sniffing inside it would go wrong; `footer` adds the ten-byte footer.
    fn id3(size: usize, footer: bool) -> Vec<u8> {
        let mut t = b"ID3\x04\x00".to_vec();
        t.push(if footer { 0x10 } else { 0 });
        t.extend((0..4).rev().map(|i| ((size >> (7 * i)) & 0x7F) as u8));
        t.extend(MP3.iter().chain(b"fLaC").cycle().take(size));
        if footer {
            t.extend(b"3DI\x04\x00\x10\x00\x00\x00\x00");
        }
        t
    }

    fn sniffed(bytes: &[u8]) -> Result<Sniffed, Error> {
        sniff(Box::new(Cursor::new(bytes.to_vec())), &Limits::default())
    }

    fn rest(s: &mut Sniffed) -> Vec<u8> {
        let mut out = Vec::new();
        read_up_to(&mut s.source, &mut out, usize::MAX).unwrap();
        out
    }

    fn input(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    #[test]
    fn magic_at_the_start_names_the_container() {
        let ftyp = b"\0\0\0\x20ftypM4A \0\0\0\0";
        for (bytes, want) in [
            (&ftyp[..], FormatId::Mp4),
            (&MP3[..], FormatId::Mp3),
            (&ADTS[..], FormatId::Adts),
            (b"fLaC\0\0\0\x22", FormatId::Flac),
            (b"OggS\0\x02", FormatId::Ogg),
        ] {
            let mut s = sniffed(bytes).unwrap();
            assert_eq!((s.format, &s.id3), (want, &None), "{bytes:02X?}");
            assert_eq!(rest(&mut s), bytes);
        }
    }

    #[test]
    fn a_tag_before_mpeg_audio_is_kept_and_the_reader_starts_after_it() {
        for footer in [false, true] {
            let tag = id3(300, footer);
            let audio = input(&[&MP3, &[7; 50]]);
            let mut s = sniffed(&input(&[&tag, &audio])).unwrap();
            assert_eq!(s.format, FormatId::Mp3);
            assert_eq!(s.id3.as_ref(), Some(&tag), "footer {footer}");
            assert_eq!(rest(&mut s), audio);
        }
    }

    #[test]
    fn tags_back_to_back_are_all_kept() {
        let (a, b) = (id3(20, false), id3(5, true));
        let audio = input(&[&ADTS, &[1, 2, 3]]);
        let mut s = sniffed(&input(&[&a, &b, &audio])).unwrap();
        assert_eq!(s.format, FormatId::Adts);
        assert_eq!(s.id3, Some(input(&[&a, &b])));
        assert_eq!(rest(&mut s), audio);
    }

    #[test]
    fn flac_behind_a_tag_gets_the_whole_input_as_before() {
        let bytes = input(&[&id3(40, false), b"fLaC\0\0\0\x22rest"]);
        let mut s = sniffed(&bytes).unwrap();
        assert_eq!((s.format, &s.id3), (FormatId::Flac, &None));
        assert_eq!(rest(&mut s), bytes);
    }

    #[test]
    fn anything_else_behind_a_tag_is_unrecognised() {
        for after in [
            &b""[..],
            b"RIFF\0\0\0\0WAVE",
            b"OggS",
            b"fLa",
            &[0xFF, 0xFB],
            b"junk",
        ] {
            let bytes = input(&[&id3(8, false), after]);
            assert_eq!(
                sniffed(&bytes).err(),
                Some(Error::UnrecognisedFormat),
                "{after:02X?}"
            );
        }
    }

    #[test]
    fn a_tag_cut_short_is_an_unexpected_eof() {
        let tag = id3(100, true);
        for cut in [3, 9, 10, 50, tag.len() - 1] {
            assert_eq!(
                sniffed(&tag[..cut]).err(),
                Some(Error::UnexpectedEof { offset: cut as u64 }),
                "{cut}"
            );
        }
    }

    #[test]
    fn a_tag_larger_than_the_allocation_limit_is_refused_before_it_is_read() {
        let limits = Limits {
            max_alloc_bytes: 1000,
            ..Limits::default()
        };
        let open = |size: usize| {
            let bytes = input(&[&id3(size, false), &MP3]);
            sniff(Box::new(Cursor::new(bytes)), &limits).map(|s| s.format)
        };
        assert_eq!(open(990), Ok(FormatId::Mp3));
        assert_eq!(
            open(991),
            Err(Error::AllocLimitExceeded {
                requested: 1001,
                max: 1000
            })
        );
        // The largest size a tag can claim, with almost nothing behind it.
        let mut huge = b"ID3\x04\x00\x10\x7F\x7F\x7F\x7F".to_vec();
        huge.extend(MP3);
        assert!(matches!(
            sniff(Box::new(Cursor::new(huge)), &limits).err(),
            Some(Error::AllocLimitExceeded { .. })
        ));
    }

    #[test]
    fn a_pipe_read_a_byte_at_a_time_sniffs_the_same() {
        let tag = id3(70, true);
        let audio = input(&[&MP3, &[9; 20]]);
        let src = FaultySource::new(Cursor::new(input(&[&tag, &audio])))
            .not_seekable()
            .max_read(1);
        let mut s = sniff(Box::new(src), &Limits::default()).unwrap();
        assert!(!s.source.is_seekable());
        assert_eq!((s.format, s.id3.as_ref()), (FormatId::Mp3, Some(&tag)));
        assert_eq!(rest(&mut s), audio);
    }

    /// Two MPEG-1 Layer III frames (417 bytes each) whose bodies hold no sync byte.
    fn two_frames() -> Vec<u8> {
        let mut f = [MP3.to_vec(), vec![0x11; 413]].concat();
        f[3] = 0x64;
        [f.clone(), f].concat()
    }

    #[test]
    fn mpeg_audio_after_leading_junk_is_found_when_a_second_frame_confirms_it() {
        for junk in [
            vec![0u8; 215],
            b"leading text".to_vec(),
            vec![0xFF; 3],
            vec![0; MAX_MPEG_JUNK],
        ] {
            let bytes = input(&[&junk, &two_frames()]);
            let mut s = sniffed(&bytes).unwrap();
            assert_eq!((s.format, &s.id3), (FormatId::Mp3, &None), "{}", junk.len());
            assert_eq!(rest(&mut s), bytes);
        }
        // Behind an ID3v2 tag too, with the tag kept apart as before.
        let tag = id3(40, false);
        let audio = input(&[&[0u8; 100], &two_frames()]);
        let mut s = sniffed(&input(&[&tag, &audio])).unwrap();
        assert_eq!((s.format, s.id3.as_ref()), (FormatId::Mp3, Some(&tag)));
        assert_eq!(rest(&mut s), audio);
    }

    #[test]
    fn junk_past_the_bound_or_an_unconfirmed_header_is_not_mpeg_audio() {
        let one = &two_frames()[..417];
        for bytes in [
            input(&[&vec![0u8; MAX_MPEG_JUNK + 1], &two_frames()]),
            // A lone header, its frame followed by something else.
            input(&[b"junk", one, b"more junk that is no header"]),
            // A free-format header cannot be confirmed by sniffing.
            input(&[
                b"junk",
                &[0xFF, 0xFB, 0x00, 0x64],
                &[0x11; 600],
                &[0xFF, 0xFB, 0x00, 0x64],
            ]),
            // Frames of two different streams back to back.
            input(&[b"j", one, &[0xFF, 0xFB, 0x94, 0x64]]),
        ] {
            assert_eq!(
                sniffed(&bytes).err(),
                Some(Error::UnrecognisedFormat),
                "{}",
                bytes.len()
            );
        }
    }

    #[test]
    fn containers_whose_data_looks_like_mpeg_frames_keep_their_magic() {
        let frames = two_frames();
        let mut wav = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        wav.extend(&frames);
        let mut flac = b"fLaC\0\0\0\x22".to_vec();
        flac.extend(&frames);
        let mut aiff = b"FORM\0\0\0\0AIFF".to_vec();
        aiff.extend(&frames);
        let mut ogg = b"OggS\0\x02".to_vec();
        ogg.extend(&frames);
        for (bytes, want) in [
            (wav, FormatId::Wav),
            (flac.clone(), FormatId::Flac),
            (aiff, FormatId::Aiff),
            (ogg, FormatId::Ogg),
            // FLAC behind a tag still goes to the FLAC reader whole.
            (input(&[&id3(10, false), &flac]), FormatId::Flac),
        ] {
            assert_eq!(sniffed(&bytes).unwrap().format, want);
        }
    }

    #[test]
    fn an_unrecognised_input_is_read_no_further_than_the_junk_window() {
        let big = vec![0x42u8; 10 * JUNK_WINDOW];
        let src = FaultySource::new(Cursor::new(big)).not_seekable();
        let mut inner: Box<dyn MediaSource> = Box::new(src);
        let mut head = Vec::new();
        read_up_to(&mut *inner, &mut head, SNIFF_LEN).unwrap();
        assert_eq!(identify(&head), None);
        assert_eq!(
            sniff(
                Box::new(FaultySource::new(Cursor::new(vec![
                    0x42u8;
                    10 * JUNK_WINDOW
                ]))),
                &Limits::default()
            )
            .err(),
            Some(Error::UnrecognisedFormat)
        );
    }

    #[test]
    fn a_header_needs_three_bytes() {
        assert_eq!(mpeg_audio(&[]), None);
        assert_eq!(mpeg_audio(&[0xFF]), None);
        assert_eq!(mpeg_audio(&[0xFF, 0xFB]), None);
        assert_eq!(mpeg_audio(&[0xFF, 0xFB, 0x90]), Some(FormatId::Mp3));
    }
}
