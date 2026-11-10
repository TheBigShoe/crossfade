//! Reading one Ogg page at a time (RFC 3533 section 6) into a buffer that is reused.

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
            assert_eq!(r.body().len(), r.lacing().iter().map(|&l| usize::from(l)).sum());
        }
        assert_eq!(seen, [(2, 0, 0), (0, 0, 1), (4, 4410, 2)]);
        assert!(!r.next_page().unwrap(), "stays ended");
    }

    #[test]
    fn page_offsets_count_from_the_start_of_the_input() {
        let page = RawPage { lacing: vec![3], body: vec![1, 2, 3], ..RawPage::default() }.to_bytes();
        let mut r = reader(&[page.clone(), page.clone()].concat());
        assert!(r.next_page().unwrap());
        assert!(r.next_page().unwrap());
        assert_eq!(r.header().offset, page.len() as u64);
    }

    #[test]
    fn short_reads_and_a_pipe_make_no_difference() {
        let src = FaultySource::new(Cursor::new(FIXTURE.to_vec())).not_seekable().max_read(1);
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
        let page = RawPage { lacing: vec![2], body: vec![7, 8], ..RawPage::default() }.to_bytes();
        let mut bytes = [page.clone(), page.clone()].concat();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        let mut r = reader(&bytes);
        assert!(r.next_page().unwrap());
        let what = malformed(r.next_page());
        assert!(what.contains("crc") && what.contains(&format!("byte {}", page.len())), "{what}");
    }

    #[test]
    fn a_page_cut_anywhere_is_an_unexpected_eof() {
        let page = RawPage { lacing: vec![2, 1], body: vec![7, 8, 9], ..RawPage::default() }.to_bytes();
        for cut in 1..page.len() {
            let got = reader(&page[..cut]).next_page();
            assert_eq!(got, Err(Error::UnexpectedEof { offset: cut as u64 }), "cut {cut}");
        }
    }

    #[test]
    fn the_biggest_page_fits() {
        let page = RawPage { lacing: vec![255; 255], body: vec![9; 255 * 255], ..RawPage::default() };
        let mut r = reader(&page.to_bytes());
        assert!(r.next_page().unwrap());
        assert_eq!(r.body().len(), 255 * 255);
        let _ = testutil::BOS;
    }
}
