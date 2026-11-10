//! Packets out of the pages of one logical stream (RFC 3533 section 5).

use super::page::PageReader;
use crossfade_core::Error;

fn malformed(what: &str) -> Error {
    Error::Malformed {
        what: what.to_owned(),
    }
}

pub(crate) struct PacketReader {
    pages: PageReader,
    serial: u32,
    max_packet: usize,
    /// `pages` holds a page nobody has looked at yet.
    pending: bool,
    next_sequence: Option<u32>,
    /// Next segment and body offset within the current page.
    seg: usize,
    off: usize,
    /// The last segment taken was 255, so the packet goes on in the next page.
    mid_packet: bool,
    /// The page with the end-of-stream flag has been loaded.
    last_page: bool,
}

impl PacketReader {
    pub fn new(pages: PageReader, serial: u32, max_packet: usize) -> Self {
        Self {
            pages,
            serial,
            max_packet,
            pending: false,
            next_sequence: None,
            seg: 0,
            off: 0,
            mid_packet: false,
            last_page: false,
        }
    }

    /// Carries on after the stream's first page, which the caller consumed itself.
    /// `page_loaded`: the reader holds a page that has not been looked at.
    pub fn resume(&mut self, next_sequence: u32, page_loaded: bool) {
        self.next_sequence = Some(next_sequence);
        self.pending = page_loaded;
        // The loaded page is not this reader's yet; its segments must not be taken unchecked.
        self.seg = self.pages.lacing().len();
    }

    /// Loads the next page of this stream; `Ok(false)` when the input ends.
    fn load_page(&mut self) -> Result<bool, Error> {
        loop {
            if self.pending {
                self.pending = false;
            } else if !self.pages.next_page()? {
                return Ok(false);
            }
            let h = *self.pages.header();
            if h.serial != self.serial {
                continue;
            }
            if h.is_first() && self.next_sequence.is_some() {
                return Err(malformed("second beginning-of-stream page for one serial"));
            }
            if let Some(want) = self.next_sequence
                && h.sequence != want
            {
                return Err(Error::Malformed {
                    what: format!(
                        "ogg page sequence jumps from {} to {} at byte {}",
                        want.wrapping_sub(1),
                        h.sequence,
                        h.offset
                    ),
                });
            }
            if h.is_continued() != self.mid_packet {
                return Err(malformed(if self.mid_packet {
                    "packet continuation is missing"
                } else {
                    "continued packet with nothing to continue"
                }));
            }
            self.next_sequence = Some(h.sequence.wrapping_add(1));
            self.last_page = h.is_last();
            self.seg = 0;
            self.off = 0;
            return Ok(true);
        }
    }

    /// Clears `out` and fills it with the next packet; `Ok(false)` at the end of the stream.
    pub fn next_packet(&mut self, out: &mut Vec<u8>) -> Result<bool, Error> {
        out.clear();
        let mut started = false;
        loop {
            if self.seg >= self.pages.lacing().len() {
                if self.last_page {
                    if self.mid_packet {
                        return Err(malformed("stream ends inside a packet"));
                    }
                    return Ok(false);
                }
                if !self.load_page()? {
                    if started || self.mid_packet {
                        return Err(Error::UnexpectedEof {
                            offset: self.pages.position(),
                        });
                    }
                    return Ok(false);
                }
                continue;
            }
            let lace = usize::from(self.pages.lacing()[self.seg]);
            if out.len() + lace > self.max_packet {
                return Err(Error::Malformed {
                    what: format!("ogg packet longer than the {} byte limit", self.max_packet),
                });
            }
            out.extend_from_slice(&self.pages.body()[self.off..self.off + lace]);
            self.off += lace;
            self.seg += 1;
            started = true;
            self.mid_packet = lace == 255;
            if lace < 255 {
                return Ok(true);
            }
        }
    }

    pub fn header(&self) -> &super::page::PageHeader {
        self.pages.header()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ogg::page::PageReader;
    use crate::ogg::testutil::{self, CONTINUED, EOS, RawPage};
    use crossfade_core::Error;
    use crossfade_testkit::io::FaultySource;
    use proptest::prelude::*;
    use std::io::Cursor;

    const BIG: usize = 1 << 20;

    fn reader(bytes: Vec<u8>, serial: u32, max: usize) -> PacketReader {
        PacketReader::new(PageReader::new(Box::new(Cursor::new(bytes))), serial, max)
    }

    fn all(r: &mut PacketReader) -> Result<Vec<Vec<u8>>, Error> {
        let mut out = Vec::new();
        let mut buf = Vec::new();
        while r.next_packet(&mut buf)? {
            out.push(buf.clone());
        }
        Ok(out)
    }

    fn malformed(r: Result<Vec<Vec<u8>>, Error>) -> String {
        match r {
            Err(Error::Malformed { what }) => what,
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    fn pkt(len: usize, fill: u8) -> Vec<u8> {
        vec![fill; len]
    }

    #[test]
    fn lacing_edge_cases_survive_every_split() {
        let packets: Vec<Vec<u8>> = [0, 1, 254, 255, 256, 510, 511, 0, 765, 0]
            .iter()
            .enumerate()
            .map(|(i, &n)| pkt(n, i as u8 + 1))
            .collect();
        for per_page in [&[255][..], &[1], &[2], &[3, 1], &[7]] {
            let bytes = testutil::bytes(&testutil::pages(9, 0, &packets, per_page));
            assert_eq!(
                all(&mut reader(bytes, 9, BIG)).unwrap(),
                packets,
                "{per_page:?}"
            );
        }
    }

    #[test]
    fn a_packet_of_exactly_255_bytes_is_followed_by_an_empty_lacing_value() {
        assert_eq!(testutil::lacing_of(255), [255, 0]);
        assert_eq!(testutil::lacing_of(510), [255, 255, 0]);
    }

    proptest! {
        #[test]
        fn arbitrary_packets_and_page_splits_read_back_identically(
            sizes in proptest::collection::vec(0usize..1200, 0..12),
            per_page in proptest::collection::vec(1usize..9, 1..5),
        ) {
            let packets: Vec<Vec<u8>> =
                sizes.iter().enumerate().map(|(i, &n)| pkt(n, i as u8)).collect();
            let bytes = testutil::bytes(&testutil::pages(3, 0, &packets, &per_page));
            prop_assert_eq!(all(&mut reader(bytes, 3, BIG)).unwrap(), packets);
        }
    }

    #[test]
    fn a_packet_can_span_pages_and_the_continued_flag_is_set() {
        let packets = vec![pkt(700, 1), pkt(10, 2)];
        let pages = testutil::pages(1, 0, &packets, &[2, 255]);
        assert!(pages[1].flags & CONTINUED != 0);
        assert_eq!(
            all(&mut reader(testutil::bytes(&pages), 1, BIG)).unwrap(),
            packets
        );
    }

    #[test]
    fn other_serial_numbers_are_ignored() {
        let a = testutil::pages(1, 0, &[pkt(300, 1), pkt(5, 2)], &[1]);
        let b = testutil::pages(2, 40, &[pkt(300, 8)], &[1]);
        let mut pages = Vec::new();
        let mut b = b.into_iter();
        for p in a {
            pages.push(p);
            pages.extend(b.next());
        }
        let got = all(&mut reader(testutil::bytes(&pages), 1, BIG)).unwrap();
        assert_eq!(got, [pkt(300, 1), pkt(5, 2)]);
    }

    #[test]
    fn a_continuation_with_nothing_to_continue_is_malformed() {
        let mut pages = testutil::pages(1, 0, &[pkt(10, 1)], &[255]);
        pages[0].flags |= CONTINUED;
        let what = malformed(all(&mut reader(testutil::bytes(&pages), 1, BIG)));
        assert!(what.contains("continu"), "{what}");
    }

    #[test]
    fn a_missing_continuation_is_malformed() {
        let mut pages = testutil::pages(1, 0, &[pkt(700, 1)], &[2, 255]);
        pages[1].flags &= !CONTINUED;
        let what = malformed(all(&mut reader(testutil::bytes(&pages), 1, BIG)));
        assert!(what.contains("continu"), "{what}");
    }

    #[test]
    fn a_sequence_gap_is_malformed() {
        let mut pages = testutil::pages(1, 0, &[pkt(10, 1), pkt(10, 2), pkt(10, 3)], &[1]);
        pages[2].sequence = 3;
        let what = malformed(all(&mut reader(testutil::bytes(&pages), 1, BIG)));
        assert!(what.contains("sequence"), "{what}");
    }

    #[test]
    fn an_over_long_packet_is_refused_without_being_buffered() {
        let bytes = testutil::bytes(&testutil::pages(1, 0, &[pkt(50_000, 1)], &[255]));
        let mut r = reader(bytes, 1, 1000);
        let mut buf = Vec::new();
        let err = r.next_packet(&mut buf).unwrap_err();
        assert!(matches!(err, Error::Malformed { .. }), "{err:?}");
        assert!(buf.capacity() <= 2048, "{}", buf.capacity());
    }

    #[test]
    fn a_packet_of_exactly_the_limit_is_fine() {
        let bytes = testutil::bytes(&testutil::pages(1, 0, &[pkt(1000, 1)], &[255]));
        assert_eq!(all(&mut reader(bytes, 1, 1000)).unwrap(), [pkt(1000, 1)]);
    }

    #[test]
    fn the_stream_ends_at_its_last_page_and_later_pages_are_ignored() {
        let mut pages = testutil::pages(1, 0, &[pkt(10, 1)], &[255]);
        let chained = testutil::pages(1, 0, &[pkt(10, 2)], &[255]);
        pages.extend(chained);
        let got = all(&mut reader(testutil::bytes(&pages), 1, BIG)).unwrap();
        assert_eq!(got, [pkt(10, 1)]);
    }

    #[test]
    fn a_packet_unfinished_at_the_last_page_is_malformed() {
        let mut page = RawPage {
            flags: EOS,
            serial: 1,
            ..RawPage::default()
        };
        page.lacing = vec![255];
        page.body = pkt(255, 1);
        let what = malformed(all(&mut reader(page.to_bytes(), 1, BIG)));
        assert!(what.contains("ends"), "{what}");
    }

    #[test]
    fn input_that_ends_inside_a_packet_is_an_unexpected_eof() {
        let mut pages = testutil::pages(1, 0, &[pkt(700, 1)], &[2, 255]);
        pages.truncate(1);
        pages[0].flags &= !EOS;
        let bytes = testutil::bytes(&pages);
        let len = bytes.len() as u64;
        assert_eq!(
            all(&mut reader(bytes, 1, BIG)),
            Err(Error::UnexpectedEof { offset: len })
        );
    }

    #[test]
    fn input_that_ends_between_packets_is_the_end() {
        let mut pages = testutil::pages(1, 0, &[pkt(10, 1), pkt(10, 2)], &[1]);
        pages.truncate(1);
        pages[0].flags &= !EOS;
        assert_eq!(
            all(&mut reader(testutil::bytes(&pages), 1, BIG)).unwrap(),
            [pkt(10, 1)]
        );
    }

    #[test]
    fn a_second_beginning_of_stream_for_the_same_serial_is_malformed() {
        let mut pages = testutil::pages(1, 0, &[pkt(10, 1), pkt(10, 2)], &[1]);
        pages[1].flags |= testutil::BOS;
        let what = malformed(all(&mut reader(testutil::bytes(&pages), 1, BIG)));
        assert!(what.contains("beginning"), "{what}");
    }

    #[test]
    fn io_errors_and_pipes_pass_through() {
        let packets = vec![pkt(300, 1), pkt(20, 2)];
        let bytes = testutil::bytes(&testutil::pages(1, 0, &packets, &[1]));
        let src = FaultySource::new(Cursor::new(bytes))
            .not_seekable()
            .max_read(7);
        let mut r = PacketReader::new(PageReader::new(Box::new(src)), 1, BIG);
        assert_eq!(all(&mut r).unwrap(), packets);
    }
}
