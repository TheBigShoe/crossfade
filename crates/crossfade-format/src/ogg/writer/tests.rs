use super::*;
use crate::ogg::crc::crc32;

const SERIAL: u32 = 0xC0FF_EE11;

struct Page {
    flags: u8,
    granule: i64,
    serial: u32,
    sequence: u32,
    lacing: Vec<u8>,
    body: Vec<u8>,
    crc_ok: bool,
}

/// An independent page parser: the writer's tests must not trust the reader under test elsewhere.
fn parse(bytes: &[u8]) -> Vec<Page> {
    let mut pages = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let h = &bytes[at..];
        assert_eq!(&h[..4], b"OggS", "page {} starts at {at}", pages.len());
        assert_eq!(h[4], 0, "stream structure version");
        let n = usize::from(h[26]);
        let lacing = h[27..27 + n].to_vec();
        let body_len: usize = lacing.iter().map(|&l| usize::from(l)).sum();
        let total = 27 + n + body_len;
        let mut copy = h[..total].to_vec();
        let stored = u32::from_le_bytes(copy[22..26].try_into().unwrap());
        copy[22..26].fill(0);
        pages.push(Page {
            flags: h[5],
            granule: i64::from_le_bytes(h[6..14].try_into().unwrap()),
            serial: u32::from_le_bytes(h[14..18].try_into().unwrap()),
            sequence: u32::from_le_bytes(h[18..22].try_into().unwrap()),
            lacing,
            body: h[27 + n..total].to_vec(),
            crc_ok: crc32(&copy) == stored,
        });
        at += total;
    }
    pages
}

/// Packets as a reader would rebuild them: `(bytes, page it ends on)`.
fn rebuild(pages: &[Page]) -> Vec<(Vec<u8>, usize)> {
    let mut out = Vec::new();
    let mut cur = Vec::new();
    for (i, p) in pages.iter().enumerate() {
        let mut at = 0;
        for &l in &p.lacing {
            cur.extend(&p.body[at..at + usize::from(l)]);
            at += usize::from(l);
            if l < 255 {
                out.push((std::mem::take(&mut cur), i));
            }
        }
    }
    assert!(cur.is_empty(), "a packet is left unterminated");
    out
}

fn bytes_of(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_add(seed)).collect()
}

fn one_packet(len: usize) -> Vec<Page> {
    let mut w = PageWriter::new(SERIAL);
    w.packet(&bytes_of(len, 7), 100);
    w.finish();
    parse(&w.take())
}

#[test]
fn lacing_for_the_boundary_lengths() {
    let cases: [(usize, &[u8]); 6] = [
        (0, &[0]),
        (1, &[1]),
        (254, &[254]),
        (255, &[255, 0]),
        (256, &[255, 1]),
        (510, &[255, 255, 0]),
    ];
    for (len, want) in cases {
        let pages = one_packet(len);
        assert_eq!(pages.len(), 1, "{len}");
        assert_eq!(pages[0].lacing, want, "{len}");
        assert_eq!(pages[0].body, bytes_of(len, 7), "{len}");
        assert!(pages[0].crc_ok, "{len}");
    }
}

#[test]
fn a_packet_longer_than_a_page_continues_on_the_next() {
    let pages = one_packet(70_000);
    // 70,000 = 274 * 255 + 130: 275 lacing values, 255 on the first page.
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].lacing, vec![255; 255]);
    assert_eq!(pages[0].granule, -1, "no packet ends on the first page");
    assert_eq!(pages[0].flags & CONTINUED, 0);
    assert_eq!(pages[1].flags & CONTINUED, CONTINUED);
    assert_eq!(pages[1].lacing.len(), 20);
    assert_eq!(*pages[1].lacing.last().unwrap(), 130);
    assert_eq!(pages[1].granule, 100);
    let packets = rebuild(&pages);
    assert_eq!(packets, [(bytes_of(70_000, 7), 1)]);
    assert!(pages.iter().all(|p| p.crc_ok));
}

#[test]
fn a_packet_of_exactly_255_full_segments_gets_its_terminator_on_the_next_page() {
    let pages = one_packet(255 * 255);
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[1].lacing, [0]);
    assert_eq!(pages[1].flags & CONTINUED, CONTINUED);
    assert_eq!(rebuild(&pages)[0].0.len(), 255 * 255);
}

#[test]
fn the_first_page_is_bos_the_last_eos_and_sequences_count_up() {
    let mut w = PageWriter::new(SERIAL);
    w.packet(&[1; 51], 0);
    w.end_page();
    w.packet(&[2; 20], 0);
    w.end_page();
    for i in 1..=40u64 {
        w.packet(&bytes_of(1000, i as u8), i * 10);
    }
    w.finish();
    let pages = parse(&w.take());
    assert!(pages.len() > 4);
    for (i, p) in pages.iter().enumerate() {
        assert_eq!(p.sequence, i as u32);
        assert_eq!(p.serial, SERIAL);
        assert!(p.crc_ok, "page {i}");
        let want = if i == 0 { BOS } else { 0 } | if i + 1 == pages.len() { EOS } else { 0 };
        assert_eq!(p.flags & (BOS | EOS), want, "page {i}");
    }
}

#[test]
fn headers_get_granule_zero_and_audio_starts_on_a_fresh_page() {
    let mut w = PageWriter::new(SERIAL);
    w.packet(&[1; 51], 0);
    w.end_page();
    w.packet(&[2; 20], 0);
    w.end_page();
    w.packet(&[3; 30], 4096);
    w.finish();
    let pages = parse(&w.take());
    assert_eq!(pages.len(), 3);
    assert_eq!([pages[0].granule, pages[1].granule], [0, 0]);
    assert_eq!(pages[2].body, [3; 30]);
    assert_eq!(pages[2].granule, 4096);
}

#[test]
fn a_page_closes_at_4_kib_or_255_segments_and_never_splits_a_small_packet() {
    let mut w = PageWriter::new(SERIAL);
    // 1,500-byte packets take 6 segments: three fit under 4 KiB, a fourth crosses it.
    for i in 1..=10u64 {
        w.packet(&bytes_of(1500, 0), i * 100);
    }
    w.finish();
    let pages = parse(&w.take());
    let per_page: Vec<usize> = pages
        .iter()
        .map(|p| p.lacing.iter().filter(|&&l| l < 255).count())
        .collect();
    assert_eq!(per_page, [3, 3, 3, 1]);
    let granules: Vec<i64> = pages.iter().map(|p| p.granule).collect();
    assert_eq!(granules, [300, 600, 900, 1000]);

    // Tiny packets: 255 segments is the limit before 4 KiB is.
    let mut w = PageWriter::new(SERIAL);
    for i in 1..=300u64 {
        w.packet(&[9; 3], i);
    }
    w.finish();
    let pages = parse(&w.take());
    assert_eq!(pages[0].lacing.len(), 255);
    assert_eq!(pages[0].granule, 255);
    assert_eq!(pages[1].granule, 300);
    assert!(pages.iter().all(|p| p.flags & CONTINUED == 0));
}

#[test]
fn a_page_that_ends_no_packet_has_granule_minus_one() {
    let mut w = PageWriter::new(SERIAL);
    w.packet(&bytes_of(3000, 1), 10);
    w.packet(&bytes_of(100_000, 2), 20);
    w.packet(&bytes_of(10, 3), 30);
    w.finish();
    let pages = parse(&w.take());
    let packets = rebuild(&pages);
    assert_eq!(packets.len(), 3);
    for (i, p) in pages.iter().enumerate() {
        let ends = p.lacing.iter().any(|&l| l < 255);
        assert_eq!(p.granule == -1, !ends, "page {i}");
    }
    assert!(pages.iter().any(|p| p.granule == -1));
    // Each page's granule is the one given for the last packet that ends on it.
    let given = [10, 20, 30];
    for (i, p) in pages.iter().enumerate() {
        if let Some(k) = packets.iter().rposition(|(_, at)| *at == i) {
            assert_eq!(p.granule, given[k]);
        }
    }
}

#[test]
fn an_empty_stream_still_ends_with_an_eos_page() {
    let mut w = PageWriter::new(SERIAL);
    w.packet(&[1; 51], 0);
    w.end_page();
    w.finish();
    let pages = parse(&w.take());
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[1].flags, EOS);
    assert!(pages[1].lacing.is_empty());
    assert_eq!(pages[1].granule, 0);
    assert!(pages[1].crc_ok);
}

#[test]
fn a_finish_right_after_a_full_page_does_not_add_an_empty_page() {
    let mut w = PageWriter::new(SERIAL);
    for i in 1..=4u64 {
        w.packet(&bytes_of(1500, 0), i);
    }
    w.finish();
    let pages = parse(&w.take());
    assert!(pages.iter().all(|p| !p.lacing.is_empty()));
    assert_eq!(pages.last().unwrap().flags & EOS, EOS);
}
