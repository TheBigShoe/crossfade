//! The Ogg FLAC mapping (RFC 9639 section 10.1).

use super::LogicalStream;
use super::packets::PacketReader;
use crate::vorbis_comment;
use crossfade_core::{Demuxer, Error, Limits, Packet, StreamInfo, Tags};
use crossfade_flac::frame::{self, HeaderError};
use crossfade_flac::streaminfo::StreamInfoBlock;

/// `0x7F "FLAC"`, version, header count, `fLaC`, block header, STREAMINFO.
const ID_LEN: usize = 5 + 2 + 2 + 4 + 4 + StreamInfoBlock::LEN;
const VORBIS_COMMENT: u8 = 4;

fn malformed(what: &str) -> Error {
    Error::Malformed {
        what: format!("ogg flac: {what}"),
    }
}

struct OggFlacDemuxer {
    packets: PacketReader,
    streams: [StreamInfo; 1],
    tags: Tags,
    /// Frames of a fixed-block-size stream are numbered in units of this.
    block_size: u32,
    /// The first audio packet, read while looking for the end of the headers.
    held: Vec<u8>,
    has_held: bool,
    done: bool,
}

pub(crate) fn open(stream: LogicalStream, limits: &Limits) -> Result<Box<dyn Demuxer>, Error> {
    let LogicalStream {
        mut packets, id, ..
    } = stream;
    let header = parse_first_packet(&id)?;
    let mut tags = None;
    let mut held = Vec::new();
    // The declared header count is only a hint: the headers end at the first frame.
    let has_held = loop {
        if !packets.next_packet(&mut held)? {
            break false;
        }
        if starts_frame(&held) {
            break true;
        }
        if tags.is_none() {
            tags = comment_tags(&held, limits)?;
        }
    };
    let tags = tags.unwrap_or_default();
    let mut stream = crate::flac::stream_info(&header.block, &header.streaminfo)?;
    crate::flac::apply_channel_mask(&mut stream, &tags)?;
    Ok(Box::new(OggFlacDemuxer {
        packets,
        streams: [stream],
        tags,
        block_size: u32::from(header.block.min_block_size),
        held,
        has_held,
        done: false,
    }))
}

impl Demuxer for OggFlacDemuxer {
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn tags(&self) -> &Tags {
        &self.tags
    }

    fn read_packet(&mut self, out: &mut Packet) -> Result<bool, Error> {
        if self.done {
            return Ok(false);
        }
        if self.has_held {
            self.has_held = false;
            std::mem::swap(&mut out.data, &mut self.held);
        } else if !self.packets.next_packet(&mut out.data)? {
            self.done = true;
            return Ok(false);
        }
        let h = frame::parse_header(&out.data).map_err(|e| Error::Malformed {
            what: match e {
                HeaderError::NeedMoreData => "ogg flac frame header is cut short".into(),
                HeaderError::Invalid(why) => format!("ogg flac frame header: {why}"),
            },
        })?;
        out.stream = 0;
        out.pts = i64::try_from(h.first_sample(self.block_size)).map_err(|_| Error::Overflow)?;
        out.duration = u64::from(h.block_size);
        Ok(true)
    }
}

pub(crate) struct MappingHeader {
    pub block: StreamInfoBlock,
    pub streaminfo: [u8; StreamInfoBlock::LEN],
}

pub(crate) fn parse_first_packet(p: &[u8]) -> Result<MappingHeader, Error> {
    if p.len() < ID_LEN {
        return Err(malformed("identification packet is short"));
    }
    if p[..5] != *b"\x7fFLAC" {
        return Err(malformed("identification packet lacks the FLAC marker"));
    }
    if p[5] != 1 {
        return Err(Error::Unsupported {
            what: format!("ogg flac mapping version {}.{}", p[5], p[6]),
        });
    }
    if p[9..13] != *b"fLaC" {
        return Err(malformed("identification packet lacks the fLaC marker"));
    }
    let len = u32::from_be_bytes([0, p[14], p[15], p[16]]);
    if p[13] & 0x7F != 0 || len as usize != StreamInfoBlock::LEN {
        return Err(malformed("first block is not a 34-byte STREAMINFO"));
    }
    let mut streaminfo = [0; StreamInfoBlock::LEN];
    streaminfo.copy_from_slice(&p[17..ID_LEN]);
    Ok(MappingHeader {
        block: StreamInfoBlock::parse(&streaminfo).map_err(crate::flac::engine_error)?,
        streaminfo,
    })
}

/// A frame header's sync code. Metadata block types stop at 126, so no header packet starts
/// with 0xFF.
pub(crate) fn starts_frame(p: &[u8]) -> bool {
    matches!(p, [0xFF, 0xF8 | 0xF9, ..])
}

/// The tags of a header packet that holds a Vorbis comment block; `None` for any other block
/// and for a comment that does not parse.
pub(crate) fn comment_tags(p: &[u8], limits: &Limits) -> Result<Option<Tags>, Error> {
    let Some((head, rest)) = p.split_first_chunk::<4>() else {
        return Err(malformed("metadata block header is cut short"));
    };
    let len = u32::from_be_bytes([0, head[1], head[2], head[3]]) as usize;
    let Some(body) = rest.get(..len) else {
        return Err(malformed("metadata block runs past its packet"));
    };
    if head[0] & 0x7F != VORBIS_COMMENT {
        return Ok(None);
    }
    Ok(vorbis_comment::parse(body, limits).ok().map(|c| c.tags))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossfade_core::{Error, Limits, Tags};

    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/ogg/stereo16.oga");

    fn fixture_id_packet() -> &'static [u8] {
        &FIXTURE[28..28 + 51]
    }

    fn id_packet(major: u8, minor: u8, count: u16, block_type: u8, len: u32) -> Vec<u8> {
        let mut p = b"\x7fFLAC".to_vec();
        p.extend([major, minor]);
        p.extend(count.to_be_bytes());
        p.extend(b"fLaC");
        p.push(block_type);
        p.extend(&len.to_be_bytes()[1..]);
        p.extend(&fixture_id_packet()[17..51]);
        p
    }

    fn malformed<T: std::fmt::Debug>(r: Result<T, Error>) -> String {
        match r {
            Err(Error::Malformed { what }) => what,
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn the_identification_packet_of_a_real_file() {
        let h = parse_first_packet(fixture_id_packet()).unwrap();
        assert_eq!(h.block.sample_rate, 44100);
        assert_eq!((h.block.channels, h.block.bits_per_sample), (2, 16));
        assert_eq!(h.block.total_samples, 4410);
        assert_eq!(h.streaminfo, fixture_id_packet()[17..]);
    }

    #[test]
    fn a_new_minor_version_is_fine_but_another_major_is_not() {
        assert!(parse_first_packet(&id_packet(1, 9, 0, 0, 34)).is_ok());
        let err = parse_first_packet(&id_packet(2, 0, 0, 0, 34))
            .map(|_| ())
            .unwrap_err();
        assert!(
            matches!(&err, Error::Unsupported { what } if what.contains("2.0")),
            "{err:?}"
        );
    }

    #[test]
    fn a_short_or_mislabelled_packet_is_malformed() {
        let good = id_packet(1, 0, 1, 0, 34);
        assert!(malformed(parse_first_packet(&good[..50]).map(|_| ())).contains("short"));
        let mut bad_magic = good.clone();
        bad_magic[0] = 0x7e;
        assert!(parse_first_packet(&bad_magic).is_err());
        let mut bad_flac = good.clone();
        bad_flac[9] = b'x';
        assert!(malformed(parse_first_packet(&bad_flac).map(|_| ())).contains("fLaC"));
        assert!(
            malformed(parse_first_packet(&id_packet(1, 0, 1, 1, 34)).map(|_| ()))
                .contains("STREAMINFO")
        );
        assert!(
            malformed(parse_first_packet(&id_packet(1, 0, 1, 0, 33)).map(|_| ()))
                .contains("STREAMINFO")
        );
        let mut zero_rate = good;
        zero_rate[27..30].fill(0);
        assert!(parse_first_packet(&zero_rate).is_err());
    }

    #[test]
    fn frames_start_with_a_sync_code() {
        assert!(starts_frame(&[0xFF, 0xF8, 0x00]));
        assert!(starts_frame(&[0xFF, 0xF9]));
        for p in [
            &[][..],
            &[0xFF],
            &[0xFF, 0xFA],
            &[0xFE, 0xF8],
            &[0x84, 0, 0, 0],
        ] {
            assert!(!starts_frame(p), "{p:?}");
        }
    }

    fn block(kind: u8, body: &[u8]) -> Vec<u8> {
        let mut p = vec![kind];
        p.extend(&(body.len() as u32).to_be_bytes()[1..]);
        p.extend(body);
        p
    }

    fn comment_body() -> Vec<u8> {
        let mut tags = Tags::new();
        tags.push("TITLE", "Song");
        let mut body = Vec::new();
        vorbis_comment::write("v", &tags, &mut body);
        body
    }

    #[test]
    fn a_vorbis_comment_block_gives_tags_and_the_last_flag_is_ignored() {
        for kind in [4, 0x84] {
            let t = comment_tags(&block(kind, &comment_body()), &Limits::default())
                .unwrap()
                .unwrap();
            assert_eq!(
                t.iter().map(|t| (&*t.key, &*t.value)).collect::<Vec<_>>(),
                [("TITLE", "Song")]
            );
        }
    }

    #[test]
    fn other_blocks_and_unparsable_comments_give_nothing() {
        let limits = Limits::default();
        assert_eq!(comment_tags(&block(1, &[0; 20]), &limits), Ok(None));
        assert_eq!(comment_tags(&block(3, &[]), &limits), Ok(None));
        assert_eq!(comment_tags(&block(4, &[1, 2, 3]), &limits), Ok(None));
    }

    #[test]
    fn a_block_length_that_overruns_the_packet_is_malformed() {
        let mut p = block(1, &[0; 10]);
        p.truncate(9);
        assert!(malformed(comment_tags(&p, &Limits::default())).contains("block"));
        assert!(malformed(comment_tags(&[4, 0], &Limits::default())).contains("block"));
    }

    mod seeking {
        use crate::ogg::testutil::{self, RawPage};
        use crossfade_core::{Demuxer, Error, Limits, Packet};
        use std::io::Cursor;

        const NOISE: &[u8] = include_bytes!("../../tests/fixtures/ogg/noise8.oga");
        const SMALL: &[u8] = include_bytes!("../../tests/fixtures/ogg/smallblock.oga");

        fn open(bytes: &[u8]) -> Box<dyn Demuxer> {
            crate::ogg::open(Box::new(Cursor::new(bytes.to_vec())), &Limits::default()).unwrap()
        }

        fn all(d: &mut dyn Demuxer) -> Result<Vec<Packet>, Error> {
            let mut out = Vec::new();
            let mut p = Packet::default();
            while d.read_packet(&mut p)? {
                out.push(p.clone());
            }
            Ok(out)
        }

        /// Every packet of the file (headers too) with the samples each one holds.
        fn packets(file: &[u8]) -> Vec<(Vec<u8>, i64)> {
            let mut pages = crate::ogg::page::PageReader::new(Box::new(Cursor::new(file.to_vec())));
            assert!(pages.next_page().unwrap());
            let serial = pages.header().serial;
            let mut r = crate::ogg::packets::PacketReader::new(
                crate::ogg::page::PageReader::new(Box::new(Cursor::new(file.to_vec()))),
                serial,
                1 << 20,
            );
            let mut out = Vec::new();
            let mut buf = Vec::new();
            while r.next_packet(&mut buf).unwrap() {
                let n = crossfade_flac::frame::parse_header(&buf)
                    .map_or(0, |h| i64::from(h.block_size));
                out.push((buf.clone(), n));
            }
            out
        }

        /// The file's packets re-paged `per_page` segments at a time (packets go on across
        /// pages), each page's granule the samples through its last completed packet, and a page
        /// of another stream after every third page.
        fn repaged(file: &[u8], per_page: &[usize]) -> Vec<u8> {
            let packets = packets(file);
            let data: Vec<Vec<u8>> = packets.iter().map(|(d, _)| d.clone()).collect();
            let mut per = vec![1];
            per.extend(per_page);
            let mut pages = testutil::pages(77, 0, &data, &per);
            let (mut done, mut samples) = (0usize, 0i64);
            for p in &mut pages {
                let ends = p.lacing.iter().filter(|&&l| l < 255).count();
                for (_, n) in &packets[done..done + ends] {
                    samples += n;
                }
                done += ends;
                p.granule = if ends == 0 { -1 } else { samples };
            }
            let mut out = Vec::new();
            for (i, p) in pages.iter().enumerate() {
                out.extend(p.to_bytes());
                if i % 3 == 2 {
                    out.extend(
                        RawPage {
                            serial: 78,
                            sequence: i as u32,
                            granule: 0,
                            lacing: vec![8],
                            body: b"OggSOggS".to_vec(),
                            ..RawPage::default()
                        }
                        .to_bytes(),
                    );
                }
            }
            out
        }

        fn check(bytes: &[u8], what: &str) {
            let from_start = all(open(bytes).as_mut()).unwrap();
            let end = from_start.last().map_or(0, |p| p.pts + p.duration as i64);
            let mut d = open(bytes);
            for target in [
                0,
                1,
                end / 2,
                end - 1,
                end,
                end + 7,
                end / 3,
                5000,
                4999,
                -1,
            ] {
                let got = d.seek(target).unwrap();
                match from_start
                    .iter()
                    .position(|p| p.pts + p.duration as i64 > target.max(0))
                {
                    Some(i) => {
                        assert_eq!(got, from_start[i].pts, "{what} {target}");
                        assert!(
                            all(d.as_mut()).unwrap() == from_start[i..],
                            "{what} {target}"
                        );
                    }
                    None => {
                        assert_eq!(got, end, "{what} {target}");
                        assert!(!d.read_packet(&mut Packet::default()).unwrap());
                    }
                }
            }
        }

        #[test]
        fn packets_continued_across_pages_and_another_stream_between_them() {
            for per_page in [&[1usize][..], &[3, 40], &[17]] {
                check(&repaged(NOISE, per_page), &format!("noise8 {per_page:?}"));
                check(
                    &repaged(SMALL, per_page),
                    &format!("smallblock {per_page:?}"),
                );
            }
        }

        #[test]
        fn granules_that_disagree_with_the_frames_still_land_at_or_before_the_target() {
            let mut bytes = repaged(SMALL, &[5]);
            // Every granule halved: the bisection lands late, and must notice.
            let mut at = 0;
            while at + 27 <= bytes.len() {
                let segs = usize::from(bytes[at + 26]);
                let body: usize = bytes[at + 27..at + 27 + segs]
                    .iter()
                    .map(|&l| usize::from(l))
                    .sum();
                let len = 27 + segs + body;
                let g = i64::from_le_bytes(bytes[at + 6..at + 14].try_into().unwrap());
                if g > 0 {
                    bytes[at + 6..at + 14].copy_from_slice(&(g / 2).to_le_bytes());
                }
                bytes[at + 22..at + 26].fill(0);
                let crc = crate::ogg::crc::crc32(&bytes[at..at + len]);
                bytes[at + 22..at + 26].copy_from_slice(&crc.to_le_bytes());
                at += len;
            }
            check(&bytes, "halved granules");
        }
    }
}
