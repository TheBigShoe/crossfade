//! The granule seek primitive against brute force over hand-built pages: interleaved streams,
//! pages that end no packet, packets across pages, capture patterns inside packets, truncated
//! files, pipes, and the number of bytes a seek reads.

use super::*;
use crate::ogg::page::PageReader;
use crate::ogg::testutil::{self, RawPage};
use crossfade_core::MediaSource;
use crossfade_testkit::io::FaultySource;
use proptest::prelude::*;
use std::io::{Cursor, SeekFrom};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const SERIAL: u32 = 5;
const BIG: usize = 1 << 20;

/// Packet bytes that hold capture patterns: a bare "OggS" and a whole page whose CRC is wrong.
fn packet(i: usize, len: usize) -> Vec<u8> {
    let mut fake = RawPage {
        serial: SERIAL,
        granule: 3,
        lacing: vec![4],
        body: b"fake".to_vec(),
        ..RawPage::default()
    }
    .to_bytes();
    fake[22] ^= 0x55;
    let mut p: Vec<u8> = [b"OggS".as_slice(), &fake]
        .concat()
        .into_iter()
        .chain((0..).map(|k| (k * 7 + i) as u8))
        .take(len)
        .collect();
    p.truncate(len);
    p
}

/// Pages of `packets` (serial 5) whose granule is the number of packets completed through the
/// page (so a page's granule is the index of the packet after it), -1 where none completes.
/// `foreign`: a page of another stream, with a capture pattern in it, after every other page.
fn build(packets: &[Vec<u8>], per_page: &[usize], foreign: bool) -> Vec<RawPage> {
    let mut ours = testutil::pages(SERIAL, 0, packets, per_page);
    let mut done = 0i64;
    for p in &mut ours {
        let ends = p.lacing.iter().filter(|&&l| l < 255).count() as i64;
        done += ends;
        p.granule = if ends == 0 { -1 } else { done };
    }
    if !foreign {
        return ours;
    }
    let mut out = Vec::new();
    for (i, p) in ours.into_iter().enumerate() {
        out.push(p);
        if i % 2 == 1 {
            out.push(RawPage {
                serial: 9,
                sequence: i as u32,
                granule: 1,
                lacing: vec![12],
                body: b"OggS\0\0OggS\0\0".to_vec(),
                ..RawPage::default()
            });
        }
    }
    out
}

/// What the seek must do: the index of the next packet and the granule it reports, from the
/// pages alone. `floor`: the packet the audio starts with, `floor_page` the page it starts on.
fn expected(
    pages: &[RawPage],
    floor: usize,
    floor_page: usize,
    target: i64,
) -> (usize, Option<i64>) {
    let best = pages
        .iter()
        .enumerate()
        .filter(|(i, p)| {
            *i >= floor_page
                && p.serial == SERIAL
                && p.granule != -1
                && p.lacing.iter().any(|&l| l < 255)
                && p.granule <= target
        })
        .map(|(_, p)| p.granule)
        .next_back();
    match best {
        Some(g) => ((g as usize).max(floor), Some(g)),
        None => (floor, None),
    }
}

fn rest(r: &mut PacketReader) -> Result<Vec<Vec<u8>>, Error> {
    let mut out = Vec::new();
    let mut buf = Vec::new();
    while r.next_packet(&mut buf)? {
        out.push(buf.clone());
    }
    Ok(out)
}

/// A reader positioned after `floor` packets, and where packet `floor` starts.
fn reader_at_floor(bytes: Vec<u8>, floor: usize) -> (PacketReader, PacketStart) {
    let mut r = PacketReader::new(PageReader::new(Box::new(Cursor::new(bytes))), SERIAL, BIG);
    let mut buf = Vec::new();
    for _ in 0..=floor {
        assert!(r.next_packet(&mut buf).unwrap());
    }
    let start = r.packet_start();
    (r, start)
}

/// The index of the page that holds the first segment of packet `n`.
fn page_of_packet(pages: &[RawPage], n: usize) -> usize {
    let mut seen = 0;
    let mut mid = false;
    for (i, p) in pages.iter().enumerate() {
        if p.serial != SERIAL {
            continue;
        }
        for &l in &p.lacing {
            if !mid && seen == n {
                return i;
            }
            mid = l == 255;
            if !mid {
                seen += 1;
            }
        }
    }
    panic!("no packet {n}");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]
    #[test]
    fn seeking_lands_after_the_last_page_at_or_before_the_target(
        sizes in proptest::collection::vec(0usize..1500, 2..40),
        per_page in proptest::collection::vec(1usize..9, 1..4),
        foreign in any::<bool>(),
        floor in 0usize..3,
        targets in proptest::collection::vec(-3i64..50, 1..5),
    ) {
        let packets: Vec<Vec<u8>> = sizes.iter().enumerate().map(|(i, &n)| packet(i, n)).collect();
        let floor = floor.min(packets.len() - 1);
        let pages = build(&packets, &per_page, foreign);
        let (mut r, start) = reader_at_floor(testutil::bytes(&pages), floor);
        let floor_page = page_of_packet(&pages, floor);
        for target in targets {
            let (next, granule) = expected(&pages, floor, floor_page, target);
            prop_assert_eq!(r.seek_granule(target, start).unwrap(), granule, "target {}", target);
            prop_assert_eq!(rest(&mut r).unwrap(), &packets[next..], "target {}", target);
        }
    }
}

#[test]
fn a_reader_on_a_pipe_cannot_seek() {
    let packets: Vec<Vec<u8>> = (0..5).map(|i| packet(i, 100)).collect();
    let bytes = testutil::bytes(&build(&packets, &[1], false));
    let src = FaultySource::new(Cursor::new(bytes)).not_seekable();
    let mut r = PacketReader::new(PageReader::new(Box::new(src)), SERIAL, BIG);
    let mut buf = Vec::new();
    assert!(r.next_packet(&mut buf).unwrap());
    let start = r.packet_start();
    assert!(matches!(
        r.seek_granule(2, start),
        Err(Error::Unsupported { .. })
    ));
}

#[test]
fn a_truncated_file_seeks_among_the_pages_that_are_whole() {
    let packets: Vec<Vec<u8>> = (0..30).map(|i| packet(i, 300)).collect();
    let pages = build(&packets, &[3], true);
    let bytes = testutil::bytes(&pages);
    // Cut inside the last few pages.
    for cut in [bytes.len() - 1, bytes.len() - 400, bytes.len() - 1300] {
        let whole: Vec<RawPage> = {
            let mut at = 0;
            pages
                .iter()
                .take_while(|p| {
                    at += p.to_bytes().len();
                    at <= cut
                })
                .cloned()
                .collect()
        };
        for target in [0, 5, 17, 25, 29, 30, 100] {
            let (mut r, start) = reader_at_floor(bytes[..cut].to_vec(), 0);
            let (next, granule) = expected(&whole, 0, 0, target);
            assert_eq!(
                r.seek_granule(target, start).unwrap(),
                granule,
                "cut {cut} target {target}"
            );
            let mut buf = Vec::new();
            if next < packets.len() && r.next_packet(&mut buf).is_ok_and(|got| got) {
                assert_eq!(buf, packets[next], "cut {cut} target {target}");
            }
        }
    }
}

/// Bytes read and seeks made through a source, readable while a reader owns it.
struct Counting {
    inner: Cursor<Vec<u8>>,
    bytes: Arc<AtomicU64>,
}

impl MediaSource for Counting {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = std::io::Read::read(&mut self.inner, buf)?;
        self.bytes.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        std::io::Seek::seek(&mut self.inner, pos)
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.inner.get_ref().len() as u64)
    }
    fn is_seekable(&self) -> bool {
        true
    }
}

/// A seek reads O(log n) pages: a long stream of 4 KiB pages with a foreign stream between them.
#[test]
fn a_seek_reads_a_logarithmic_number_of_pages() {
    let packets: Vec<Vec<u8>> = (0..40_000).map(|i| packet(i, 200 + i % 300)).collect();
    let pages = build(&packets, &[16, 9, 23], true);
    let max_page = pages.iter().map(|p| p.to_bytes().len()).max().unwrap() as u64;
    let bytes = testutil::bytes(&pages);
    let len = bytes.len() as u64;
    let counter = Arc::new(AtomicU64::new(0));
    let src = Counting {
        inner: Cursor::new(bytes),
        bytes: counter.clone(),
    };
    let mut r = PacketReader::new(PageReader::new(Box::new(src)), SERIAL, BIG);
    let mut buf = Vec::new();
    assert!(r.next_packet(&mut buf).unwrap());
    let start = r.packet_start();
    let log2 = 64 - u64::from(len.leading_zeros());
    let mut worst = 0;
    for k in 0..50i64 {
        let target = (k * 7919) % 40_000;
        counter.swap(0, Ordering::Relaxed);
        let g = r.seek_granule(target, start).unwrap();
        assert!(g.map_or(target < 50, |g| g <= target), "{target}: {g:?}");
        worst = worst.max(counter.swap(0, Ordering::Relaxed));
    }
    eprintln!("{len} byte file, pages up to {max_page} bytes: worst seek read {worst} bytes");
    assert!(worst <= 4 * max_page * log2, "{worst}");
}
