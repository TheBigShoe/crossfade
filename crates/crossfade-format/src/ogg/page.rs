//! Reading one Ogg page at a time (RFC 3533 section 6) into a buffer that is reused.

use super::crc::{crc32, crc32_update};
use crossfade_core::{Error, MediaSource};
use std::io::ErrorKind;

const HEADER_LEN: usize = 27;
const MAX_PAGE: usize = HEADER_LEN + 255 + 255 * 255;

const CONTINUED: u8 = 0x01;
const FIRST: u8 = 0x02;
const LAST: u8 = 0x04;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PageHeader {
    pub(crate) flags: u8,
    /// Timing comes from the frame headers; only the tests read this.
    #[allow(dead_code)]
    pub(crate) granule: i64,
    pub(crate) serial: u32,
    pub(crate) sequence: u32,
    /// Byte offset of the page in the input, for error messages.
    pub(crate) offset: u64,
}

impl PageHeader {
    pub(crate) fn is_continued(&self) -> bool {
        self.flags & CONTINUED != 0
    }

    pub(crate) fn is_first(&self) -> bool {
        self.flags & FIRST != 0
    }

    pub(crate) fn is_last(&self) -> bool {
        self.flags & LAST != 0
    }
}

pub(crate) struct PageReader {
    source: Box<dyn MediaSource>,
    pos: u64,
    /// The whole page: header, lacing, body. Allocated once at the largest page size.
    buf: Vec<u8>,
    header: PageHeader,
    segments: usize,
    body_len: usize,
}

fn malformed(what: String) -> Error {
    Error::Malformed { what }
}

impl PageReader {
    pub(crate) fn new(source: Box<dyn MediaSource>) -> Self {
        Self {
            source,
            pos: 0,
            buf: vec![0; MAX_PAGE],
            header: PageHeader::default(),
            segments: 0,
            body_len: 0,
        }
    }

    pub(crate) fn header(&self) -> &PageHeader {
        &self.header
    }

    pub(crate) fn lacing(&self) -> &[u8] {
        &self.buf[HEADER_LEN..HEADER_LEN + self.segments]
    }

    pub(crate) fn body(&self) -> &[u8] {
        let start = HEADER_LEN + self.segments;
        &self.buf[start..start + self.body_len]
    }

    /// Bytes read so far.
    pub(crate) fn position(&self) -> u64 {
        self.pos
    }

    /// Reads into `buf[from..to]`; returns how many bytes arrived before the input ended.
    fn fill(&mut self, from: usize, to: usize) -> Result<usize, Error> {
        let mut n = from;
        while n < to {
            match self.source.read(&mut self.buf[n..to]) {
                Ok(0) => break,
                Ok(k) => {
                    let k = k.min(to - n);
                    n += k;
                    self.pos = self.pos.saturating_add(k as u64);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(n - from)
    }

    fn fill_exact(&mut self, from: usize, to: usize) -> Result<(), Error> {
        if self.fill(from, to)? < to - from {
            return Err(Error::UnexpectedEof { offset: self.pos });
        }
        Ok(())
    }

    /// `Ok(false)` when the input ends cleanly between pages.
    pub(crate) fn next_page(&mut self) -> Result<bool, Error> {
        let offset = self.pos;
        match self.fill(0, HEADER_LEN)? {
            0 => return Ok(false),
            n if n < HEADER_LEN => return Err(Error::UnexpectedEof { offset: self.pos }),
            _ => {}
        }
        if self.buf[..4] != *b"OggS" {
            return Err(malformed(format!("no Ogg page at byte {offset}")));
        }
        if self.buf[4] != 0 {
            return Err(malformed(format!(
                "Ogg page version {} at byte {offset}",
                self.buf[4]
            )));
        }
        let segments = usize::from(self.buf[26]);
        self.fill_exact(HEADER_LEN, HEADER_LEN + segments)?;
        let body_len: usize = self.buf[HEADER_LEN..HEADER_LEN + segments]
            .iter()
            .map(|&l| usize::from(l))
            .sum();
        let end = HEADER_LEN + segments + body_len;
        self.fill_exact(HEADER_LEN + segments, end)?;

        let stored = u32::from_le_bytes([self.buf[22], self.buf[23], self.buf[24], self.buf[25]]);
        let crc = crc32_update(
            crc32_update(crc32(&self.buf[..22]), &[0; 4]),
            &self.buf[26..end],
        );
        if crc != stored {
            return Err(malformed(format!("Ogg page crc mismatch at byte {offset}")));
        }
        let b = &self.buf;
        self.header = PageHeader {
            flags: b[5],
            granule: i64::from_le_bytes(b[6..14].try_into().expect("8 bytes")),
            serial: u32::from_le_bytes(b[14..18].try_into().expect("4 bytes")),
            sequence: u32::from_le_bytes(b[18..22].try_into().expect("4 bytes")),
            offset,
        };
        self.segments = segments;
        self.body_len = body_len;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ogg::testutil::{self, RawPage};
    use crossfade_core::Error;
    use crossfade_testkit::io::FaultySource;
    use std::io::Cursor;

    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/ogg/stereo16.oga");

    fn reader(bytes: &[u8]) -> PageReader {
        PageReader::new(Box::new(Cursor::new(bytes.to_vec())))
    }

    fn malformed(r: Result<bool, Error>) -> String {
        match r {
            Err(Error::Malformed { what }) => what,
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn the_first_page_of_a_real_file() {
        let mut r = reader(FIXTURE);
        assert!(r.next_page().unwrap());
        let h = r.header();
        assert_eq!(h.flags, 0x02);
        assert!(h.is_first() && !h.is_last() && !h.is_continued());
        assert_eq!((h.granule, h.serial, h.sequence, h.offset), (0, 1001, 0, 0));
        assert_eq!(r.lacing(), [51]);
        assert_eq!(&r.body()[..5], b"\x7fFLAC");
    }

    #[test]
    fn a_whole_file_is_a_run_of_pages_then_a_clean_end() {
        let mut r = reader(FIXTURE);
        let mut seen = Vec::new();
        while r.next_page().unwrap() {
            let h = r.header();
            seen.push((h.flags, h.granule, h.sequence));
            assert_eq!(
                r.body().len(),
                r.lacing().iter().map(|&l| usize::from(l)).sum()
            );
        }
        assert_eq!(seen, [(2, 0, 0), (0, 0, 1), (4, 4410, 2)]);
        assert!(!r.next_page().unwrap(), "stays ended");
    }

    #[test]
    fn page_offsets_count_from_the_start_of_the_input() {
        let page = RawPage {
            lacing: vec![3],
            body: vec![1, 2, 3],
            ..RawPage::default()
        }
        .to_bytes();
        let mut r = reader(&[page.clone(), page.clone()].concat());
        assert!(r.next_page().unwrap());
        assert!(r.next_page().unwrap());
        assert_eq!(r.header().offset, page.len() as u64);
    }

    #[test]
    fn short_reads_and_a_pipe_make_no_difference() {
        let src = FaultySource::new(Cursor::new(FIXTURE.to_vec()))
            .not_seekable()
            .max_read(1);
        let mut r = PageReader::new(Box::new(src));
        let mut n = 0;
        while r.next_page().unwrap() {
            n += 1;
        }
        assert_eq!(n, 3);
    }

    #[test]
    fn an_empty_input_has_no_pages() {
        assert!(!reader(&[]).next_page().unwrap());
    }

    #[test]
    fn bytes_that_are_not_a_page_are_malformed_with_the_offset() {
        let page = RawPage::default().to_bytes();
        let mut bytes = page.clone();
        bytes.extend(b"not a page at all, but long enough");
        let mut r = reader(&bytes);
        assert!(r.next_page().unwrap());
        let what = malformed(r.next_page());
        assert!(what.contains(&format!("byte {}", page.len())), "{what}");
    }

    #[test]
    fn a_version_other_than_zero_is_malformed() {
        let mut bytes = RawPage::default().to_bytes();
        bytes[4] = 1;
        assert!(malformed(reader(&bytes).next_page()).contains("version"));
    }

    #[test]
    fn a_wrong_crc_is_malformed_and_names_the_page() {
        let page = RawPage {
            lacing: vec![2],
            body: vec![7, 8],
            ..RawPage::default()
        }
        .to_bytes();
        let mut bytes = [page.clone(), page.clone()].concat();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        let mut r = reader(&bytes);
        assert!(r.next_page().unwrap());
        let what = malformed(r.next_page());
        assert!(
            what.contains("crc") && what.contains(&format!("byte {}", page.len())),
            "{what}"
        );
    }

    #[test]
    fn a_page_cut_anywhere_is_an_unexpected_eof() {
        let page = RawPage {
            lacing: vec![2, 1],
            body: vec![7, 8, 9],
            ..RawPage::default()
        }
        .to_bytes();
        for cut in 1..page.len() {
            let got = reader(&page[..cut]).next_page();
            assert_eq!(
                got,
                Err(Error::UnexpectedEof { offset: cut as u64 }),
                "cut {cut}"
            );
        }
    }

    #[test]
    fn the_biggest_page_fits() {
        let page = RawPage {
            lacing: vec![255; 255],
            body: vec![9; 255 * 255],
            ..RawPage::default()
        };
        let mut r = reader(&page.to_bytes());
        assert!(r.next_page().unwrap());
        assert_eq!(r.body().len(), 255 * 255);
        let _ = testutil::BOS;
    }
}
