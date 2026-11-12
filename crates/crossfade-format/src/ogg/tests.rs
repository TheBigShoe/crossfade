//! The Ogg FLAC demuxer against the reference encoder's files and hand-built streams.

use super::testutil::{self, CONTINUED, RawPage};
use super::*;
use crate::ogg::packets::PacketReader;
use crate::ogg::page::PageReader;
use crate::{FormatId, open_demuxer};
use crossfade_core::{CodecId, Packet, Tags};
use crossfade_flac::crc::crc16;
use crossfade_flac::frame::{self, ChannelAssignment, FrameHeader};
use crossfade_flac::streaminfo::StreamInfoBlock;
use crossfade_testkit::io::FaultySource;
use std::io::Cursor;

macro_rules! fixture {
    ($name:literal) => {
        include_bytes!(concat!("../../tests/fixtures/ogg/", $name))
    };
}

const NAMES: [&str; 5] = ["stereo16", "mono24", "tagged", "smallblock", "noise8"];

fn oga(name: &str) -> &'static [u8] {
    match name {
        "stereo16" => fixture!("stereo16.oga"),
        "mono24" => fixture!("mono24.oga"),
        "tagged" => fixture!("tagged.oga"),
        "smallblock" => fixture!("smallblock.oga"),
        "noise8" => fixture!("noise8.oga"),
        "serial" => fixture!("serial.oga"),
        _ => panic!("no fixture {name}"),
    }
}

fn twin(name: &str) -> &'static [u8] {
    match name {
        "stereo16" => fixture!("stereo16.flac"),
        "mono24" => fixture!("mono24.flac"),
        "tagged" => fixture!("tagged.flac"),
        "smallblock" => fixture!("smallblock.flac"),
        "noise8" => fixture!("noise8.flac"),
        _ => panic!("no twin {name}"),
    }
}

struct Native {
    block: StreamInfoBlock,
    tags: Tags,
    frames: Vec<Vec<u8>>,
}

/// Splits a native FLAC file on frame boundaries: a frame ends where the next valid header
/// starts and the bytes between check out against the trailing CRC-16 (residue zero).
fn native(file: &[u8]) -> Native {
    assert_eq!(&file[..4], b"fLaC");
    let mut at = 4;
    let (mut block, mut tags) = (None, Tags::new());
    loop {
        let (kind, last) = (file[at] & 0x7F, file[at] & 0x80 != 0);
        let len = u32::from_be_bytes([0, file[at + 1], file[at + 2], file[at + 3]]) as usize;
        let body = &file[at + 4..at + 4 + len];
        match kind {
            0 => block = Some(StreamInfoBlock::parse(body).unwrap()),
            4 => {
                tags = crate::vorbis_comment::parse(body, &Limits::default())
                    .unwrap()
                    .tags;
            }
            _ => {}
        }
        at += 4 + len;
        if last {
            break;
        }
    }
    let mut frames = Vec::new();
    let mut start = at;
    let mut q = start + 1;
    while start < file.len() {
        let is_end = q >= file.len()
            || (frame::parse_header(&file[q..]).is_ok() && crc16(&file[start..q]) == 0);
        if is_end {
            frames.push(file[start..q.min(file.len())].to_vec());
            start = q;
            q = start + 1;
        } else {
            q += 1;
        }
    }
    Native {
        block: block.unwrap(),
        tags,
        frames,
    }
}

fn open_bytes(bytes: Vec<u8>) -> Result<Box<dyn Demuxer>, Error> {
    open(Box::new(Cursor::new(bytes)), &Limits::default())
}

fn read_all(d: &mut dyn Demuxer) -> Result<Vec<Packet>, Error> {
    let mut out = Vec::new();
    let mut p = Packet::default();
    while d.read_packet(&mut p)? {
        out.push(p.clone());
    }
    Ok(out)
}

fn malformed<T: std::fmt::Debug>(r: Result<T, Error>) -> String {
    match r {
        Err(Error::Malformed { what }) => what,
        other => panic!("expected Malformed, got {other:?}"),
    }
}

/// Every packet of a file, headers included, through the packet reader alone.
fn raw_packets(file: &[u8]) -> (u32, Vec<Vec<u8>>) {
    let mut pages = PageReader::new(Box::new(Cursor::new(file.to_vec())));
    assert!(pages.next_page().unwrap());
    let serial = pages.header().serial;
    let mut r = PacketReader::new(
        PageReader::new(Box::new(Cursor::new(file.to_vec()))),
        serial,
        1 << 20,
    );
    let mut out = Vec::new();
    let mut buf = Vec::new();
    while r.next_packet(&mut buf).unwrap() {
        out.push(buf.clone());
    }
    (serial, out)
}

/// The fixture's packets as pages: the identification packet alone on the first page.
fn stream_pages(name: &str, serial: u32, per_page: &[usize]) -> Vec<RawPage> {
    let (_, packets) = raw_packets(oga(name));
    let mut per = vec![1];
    per.extend(per_page);
    testutil::pages(serial, 0, &packets, &per)
}

#[test]
fn every_fixture_yields_the_frames_of_its_native_twin() {
    for name in NAMES {
        let twin = native(twin(name));
        let mut d = open_bytes(oga(name).to_vec()).unwrap();
        let got = read_all(d.as_mut()).unwrap();
        assert_eq!(got.len(), twin.frames.len(), "{name}");
        let mut next = 0u64;
        for (p, frame) in got.iter().zip(&twin.frames) {
            assert_eq!(&p.data, frame, "{name}");
            assert_eq!((p.stream, p.pts), (0, next as i64), "{name}");
            let h = frame::parse_header(frame).unwrap();
            assert_eq!(p.duration, u64::from(h.block_size), "{name}");
            next += p.duration;
        }
        assert_eq!(next, twin.block.total_samples, "{name}");
        assert!(
            !d.read_packet(&mut Packet::default()).unwrap(),
            "{name} stays ended"
        );
    }
}

#[test]
fn the_stream_and_tags_equal_the_twins() {
    for name in NAMES {
        let twin = native(twin(name));
        let d = open_bytes(oga(name).to_vec()).unwrap();
        let want = crate::flac::stream_info(&twin.block, &twin.block.write()).unwrap();
        assert_eq!(d.streams(), [want], "{name}");
        assert_eq!(d.streams()[0].codec, CodecId::Flac);
        assert_eq!(d.tags(), &twin.tags, "{name}");
    }
    let d = open_bytes(oga("tagged").to_vec()).unwrap();
    assert_eq!(d.tags().get("TITLE"), Some("Crossfade"));
    assert_eq!(d.tags().get("ARTIST"), Some("Tester"));
    assert_eq!(d.tags().len(), 3);
    assert!(
        open_bytes(oga("stereo16").to_vec())
            .unwrap()
            .tags()
            .is_empty()
    );
}

#[test]
fn a_page_granule_is_the_sample_count_through_its_last_completed_packet() {
    for name in NAMES {
        let mut d = open_bytes(oga(name).to_vec()).unwrap();
        let audio = read_all(d.as_mut()).unwrap();
        let (_, all) = raw_packets(oga(name));
        let headers = all.len() - audio.len();
        let mut pages = PageReader::new(Box::new(Cursor::new(oga(name).to_vec())));
        let mut completed = 0usize;
        while pages.next_page().unwrap() {
            let done = pages.lacing().iter().filter(|&&l| l < 255).count();
            completed += done;
            let want = if done == 0 {
                -1
            } else if completed <= headers {
                0
            } else {
                let p = &audio[completed - headers - 1];
                (p.pts as u64 + p.duration) as i64
            };
            assert_eq!(
                pages.header().granule,
                want,
                "{name} page {}",
                pages.header().sequence
            );
        }
    }
}

#[test]
fn many_packets_share_a_page_in_the_small_block_file() {
    let mut pages = PageReader::new(Box::new(Cursor::new(oga("smallblock").to_vec())));
    let mut most = 0;
    while pages.next_page().unwrap() {
        most = most.max(pages.lacing().len());
    }
    assert!(most >= 10, "{most}");
}

#[test]
fn a_noise_packet_that_spans_pages_is_reassembled() {
    let pages = stream_pages("noise8", 5, &[20]);
    assert!(pages.iter().any(|p| p.flags & CONTINUED != 0));
    let mut d = open_bytes(testutil::bytes(&pages)).unwrap();
    let got: Vec<_> = read_all(d.as_mut())
        .unwrap()
        .into_iter()
        .map(|p| p.data)
        .collect();
    assert_eq!(got, native(twin("noise8")).frames);
}

#[test]
fn a_pipe_works_and_so_do_one_byte_reads() {
    let src = FaultySource::new(Cursor::new(oga("tagged").to_vec()))
        .not_seekable()
        .max_read(1);
    let mut d = open(Box::new(src), &Limits::default()).unwrap();
    assert_eq!(
        read_all(d.as_mut()).unwrap().len(),
        native(twin("tagged")).frames.len()
    );
}

#[test]
fn the_format_sniffer_routes_ogg_here() {
    let src = Box::new(Cursor::new(oga("stereo16").to_vec()));
    let (id, d) = open_demuxer(src, &Limits::default()).unwrap();
    assert_eq!(id, FormatId::Ogg);
    assert_eq!(d.streams()[0].codec, CodecId::Flac);
}

#[test]
fn cutting_a_file_anywhere_never_panics_and_only_whole_files_are_complete() {
    let file = oga("smallblock");
    let whole = native(twin("smallblock")).frames.len();
    for cut in 0..=file.len() {
        let src = FaultySource::new(Cursor::new(file.to_vec())).truncate_at(cut as u64);
        let got = open(Box::new(src), &Limits::default()).and_then(|mut d| read_all(d.as_mut()));
        match got {
            Ok(p) => assert!(cut == file.len() || p.len() < whole, "cut {cut}"),
            Err(e) => assert!(
                matches!(
                    e,
                    Error::UnexpectedEof { .. }
                        | Error::UnrecognisedFormat
                        | Error::Malformed { .. }
                ),
                "{e:?}"
            ),
        }
    }
}

fn fake_first_packet(prefix: &[u8]) -> Vec<u8> {
    let mut p = prefix.to_vec();
    p.resize(30, 0);
    p
}

#[test]
fn another_stream_in_the_same_file_is_skipped_wherever_it_sits() {
    let flac = stream_pages("stereo16", 1001, &[255]);
    let theora = testutil::pages(
        77,
        0,
        &[fake_first_packet(b"\x80theora"), vec![5; 600]],
        &[1, 2],
    );
    for flac_first in [true, false] {
        let (a, b) = if flac_first {
            (&flac, &theora)
        } else {
            (&theora, &flac)
        };
        let mut mixed = vec![a[0].clone(), b[0].clone()];
        let (mut ra, mut rb) = (a[1..].iter(), b[1..].iter());
        loop {
            let (x, y) = (ra.next(), rb.next());
            if x.is_none() && y.is_none() {
                break;
            }
            mixed.extend(x.cloned());
            mixed.extend(y.cloned());
        }
        let mut d = open_bytes(testutil::bytes(&mixed)).unwrap();
        let got: Vec<_> = read_all(d.as_mut())
            .unwrap()
            .into_iter()
            .map(|p| p.data)
            .collect();
        assert_eq!(
            got,
            native(twin("stereo16")).frames,
            "flac first: {flac_first}"
        );
    }
}

#[test]
fn the_first_flac_stream_wins_when_there_are_two() {
    let a = stream_pages("stereo16", 1001, &[255]);
    let b = stream_pages("tagged", 3003, &[255]);
    let mixed = [
        vec![a[0].clone(), b[0].clone()],
        a[1..].to_vec(),
        b[1..].to_vec(),
    ]
    .concat();
    let d = open_bytes(testutil::bytes(&mixed)).unwrap();
    assert!(d.tags().is_empty());
}

#[test]
fn vorbis_and_opus_are_recognised_and_handed_to_their_mappings() {
    for (prefix, name) in [(&b"\x01vorbis"[..], "vorbis"), (b"OpusHead", "opus")] {
        let pages = testutil::pages(1, 0, &[fake_first_packet(prefix), vec![1; 10]], &[1, 255]);
        match open_bytes(testutil::bytes(&pages)) {
            Err(Error::Unsupported { what }) => assert_eq!(
                what,
                format!("{name} in ogg is recognised, but reading it is not built yet")
            ),
            other => panic!("{name}: {:?}", other.err()),
        }
    }
}

/// The first stream, in page order, whose codec has a mapping is the one read.
#[test]
fn the_first_flac_vorbis_or_opus_stream_is_chosen() {
    let first = |serial: u32, prefix: &[u8]| {
        testutil::pages(serial, 0, &[fake_first_packet(prefix), vec![1; 10]], &[1, 255])
    };
    let flac = stream_pages("stereo16", 1001, &[255]);
    let skeleton = first(1, b"fishead\0");
    let theora = first(2, b"\x80theora");
    let vorbis = first(3, b"\x01vorbis");
    let opus = first(4, b"OpusHead");
    let bos = |streams: &[&Vec<RawPage>]| {
        let mut pages: Vec<RawPage> = streams.iter().map(|s| s[0].clone()).collect();
        pages.extend(streams.iter().flat_map(|s| s[1..].iter().cloned()));
        open_bytes(testutil::bytes(&pages)).map(|d| d.streams()[0].codec)
    };
    let refused = |name: &str| {
        Err(Error::Unsupported {
            what: format!("{name} in ogg is recognised, but reading it is not built yet"),
        })
    };
    assert_eq!(bos(&[&skeleton, &vorbis, &flac]), refused("vorbis"));
    assert_eq!(bos(&[&theora, &opus, &vorbis, &flac]), refused("opus"));
    assert_eq!(bos(&[&skeleton, &theora, &flac, &opus]), Ok(CodecId::Flac));
}

#[test]
fn the_chosen_stream_reaches_its_mapping_after_the_beginning_pages() {
    let theora = testutil::pages(5, 0, &[fake_first_packet(b"\x80theora"), vec![9; 40]], &[1]);
    let id = fake_first_packet(b"\x01vorbis");
    let vorbis = testutil::pages(6, 0, &[id.clone(), vec![1; 300], vec![2; 7]], &[1, 2]);
    let pages = [
        theora[0].clone(),
        vorbis[0].clone(),
        theora[1].clone(),
        vorbis[1].clone(),
    ];
    let (mapping, mut stream) =
        choose(Box::new(Cursor::new(testutil::bytes(&pages))), &Limits::default()).unwrap();
    assert_eq!(mapping, Mapping::Vorbis);
    assert_eq!((stream.serial, &stream.id), (6, &id));
    let mut p = Vec::new();
    assert!(stream.packets.next_packet(&mut p).unwrap());
    assert_eq!(p, [1; 300]);
    assert!(stream.packets.next_packet(&mut p).unwrap());
    assert_eq!(p, [2; 7]);
    assert!(!stream.packets.next_packet(&mut p).unwrap());
}

#[test]
fn a_vorbis_first_page_with_more_than_its_identification_packet_is_malformed() {
    let packets = [fake_first_packet(b"\x01vorbis"), vec![1; 10]];
    let pages = testutil::pages(1, 0, &packets, &[2]);
    assert_eq!(
        malformed(open_bytes(testutil::bytes(&pages)).map(|_| ())),
        "ogg vorbis first page holds more than the identification packet"
    );
}

#[test]
fn a_stream_that_is_not_flac_vorbis_or_opus_is_refused_by_name() {
    for (prefix, name) in [
        (&b"\x80theora"[..], "theora"),
        (b"Speex   ", "speex"),
        (b"fishead\0", "skeleton"),
        (b"mystery!", "unknown"),
    ] {
        let pages = testutil::pages(1, 0, &[fake_first_packet(prefix), vec![1; 10]], &[1, 255]);
        match open_bytes(testutil::bytes(&pages)) {
            Err(Error::Unsupported { what }) => assert_eq!(
                what,
                format!("ogg stream holds {name}; only flac, vorbis and opus in ogg can be read")
            ),
            other => panic!("{name}: {:?}", other.err()),
        }
    }
}

#[test]
fn a_chained_stream_ends_the_input() {
    let first = stream_pages("stereo16", 1001, &[255]);
    let second = stream_pages("tagged", 3003, &[255]);
    let mut d = open_bytes(testutil::bytes(&[first, second].concat())).unwrap();
    assert_eq!(
        read_all(d.as_mut()).unwrap().len(),
        native(twin("stereo16")).frames.len()
    );
}

#[test]
fn a_different_serial_number_changes_nothing() {
    let mut d = open_bytes(oga("serial").to_vec()).unwrap();
    let a = read_all(d.as_mut()).unwrap();
    let mut d = open_bytes(oga("stereo16").to_vec()).unwrap();
    assert_eq!(a, read_all(d.as_mut()).unwrap());
}

/// Found by the `ogg_demux` fuzz target: skipping a page of another stream at the end of input
/// left that page's segments looking like ours, and the next read sliced past its body.
#[test]
fn a_foreign_page_at_the_end_of_input_ends_the_stream_without_a_panic() {
    let mut d = open_bytes(fixture!("foreign-serial-tail.oga").to_vec()).unwrap();
    assert!(read_all(d.as_mut()).unwrap().is_empty());
    let mut p = Packet::default();
    assert!(!d.read_packet(&mut p).unwrap());
}

#[test]
fn a_file_that_does_not_start_a_stream_is_malformed() {
    let mut pages = stream_pages("stereo16", 1, &[255]);
    pages[0].flags &= !testutil::BOS;
    assert!(malformed(open_bytes(testutil::bytes(&pages)).map(|_| ())).contains("beginning"));
}

#[test]
fn a_version_two_mapping_is_unsupported() {
    let mut pages = stream_pages("stereo16", 1, &[255]);
    pages[0].body[5] = 2;
    let err = open_bytes(testutil::bytes(&pages)).map(|_| ()).unwrap_err();
    assert!(matches!(err, Error::Unsupported { .. }), "{err:?}");
}

#[test]
fn a_first_page_that_holds_more_than_the_identification_packet_is_malformed() {
    let (_, packets) = raw_packets(oga("stereo16"));
    let pages = testutil::pages(1, 0, &packets, &[255]);
    assert!(open_bytes(testutil::bytes(&pages)).is_err());
}

#[test]
fn the_declared_header_count_is_not_trusted() {
    for declared in [0u16, 1, 9] {
        let mut pages = stream_pages("tagged", 1, &[255]);
        pages[0].body[7..9].copy_from_slice(&declared.to_be_bytes());
        let d = open_bytes(testutil::bytes(&pages)).unwrap();
        assert_eq!(d.tags().len(), 3, "{declared}");
    }
}

#[test]
fn extra_header_blocks_are_skipped_and_the_first_comment_is_kept() {
    let (_, mut packets) = raw_packets(oga("tagged"));
    let frames = packets.split_off(2);
    let mut padding = vec![1, 0, 0, 20];
    padding.resize(24, 0);
    let mut tags = Tags::new();
    tags.push("TITLE", "Other");
    let mut body = Vec::new();
    crate::vorbis_comment::write("v", &tags, &mut body);
    let mut other = vec![4];
    other.extend(&(body.len() as u32).to_be_bytes()[1..]);
    other.extend(body);
    let all = [packets, vec![padding, other], frames].concat();
    let pages = testutil::pages(1, 0, &all, &[1, 255]);
    let mut d = open_bytes(testutil::bytes(&pages)).unwrap();
    assert_eq!(d.tags().get("TITLE"), Some("Crossfade"));
    assert_eq!(
        read_all(d.as_mut()).unwrap().len(),
        native(twin("tagged")).frames.len()
    );
}

#[test]
fn a_wrong_page_crc_names_the_page_and_packets_before_it_still_arrive() {
    let pages = stream_pages("smallblock", 1, &[4]);
    let mut bytes = testutil::bytes(&pages);
    let at: usize = pages[..pages.len() - 1]
        .iter()
        .map(|p| p.to_bytes().len())
        .sum();
    bytes[at + 30] ^= 0x55;
    let mut d = open_bytes(bytes).unwrap();
    let what = malformed(read_all(d.as_mut()));
    assert!(
        what.contains("crc") && what.contains(&format!("byte {at}")),
        "{what}"
    );
}

#[test]
fn a_wrong_crc_on_the_first_page_fails_the_open() {
    let mut bytes = oga("stereo16").to_vec();
    bytes[40] ^= 1;
    assert!(malformed(open_bytes(bytes).map(|_| ())).contains("crc"));
}

#[test]
fn a_sequence_gap_is_malformed() {
    let mut pages = stream_pages("smallblock", 1, &[4]);
    let n = pages.len();
    pages[n - 1].sequence += 1;
    let mut d = open_bytes(testutil::bytes(&pages)).unwrap();
    assert!(malformed(read_all(d.as_mut())).contains("sequence"));
}

#[test]
fn an_orphan_continuation_is_malformed() {
    let mut pages = stream_pages("smallblock", 1, &[3]);
    // A page that starts a packet, so the flag has nothing to continue.
    let k = (2..pages.len())
        .find(|&k| pages[k].flags & CONTINUED == 0)
        .unwrap();
    pages[k].flags |= CONTINUED;
    let r = open_bytes(testutil::bytes(&pages)).and_then(|mut d| read_all(d.as_mut()));
    assert!(malformed(r).contains("continu"));
}

#[test]
fn an_audio_packet_that_is_not_a_frame_is_malformed() {
    let (_, mut packets) = raw_packets(oga("stereo16"));
    // A frame header with its CRC-8 spoiled.
    let n = packets.len() - 1;
    packets[n][4] ^= 0xFF;
    let pages = testutil::pages(1, 0, &packets, &[1, 255]);
    let mut d = open_bytes(testutil::bytes(&pages)).unwrap();
    assert!(malformed(read_all(d.as_mut())).contains("frame header"));
}

#[test]
fn a_frame_header_cut_short_is_malformed() {
    let (_, mut packets) = raw_packets(oga("stereo16"));
    let n = packets.len() - 1;
    packets[n].truncate(3);
    let pages = testutil::pages(1, 0, &packets, &[1, 255]);
    let mut d = open_bytes(testutil::bytes(&pages)).unwrap();
    assert!(malformed(read_all(d.as_mut())).contains("frame header"));
}

#[test]
fn a_small_packet_limit_refuses_the_big_frames() {
    let limits = Limits {
        max_packet_bytes: 4000,
        ..Limits::default()
    };
    // The first frame is read while the headers are searched for their end, so it fails the open.
    let got = open(Box::new(Cursor::new(oga("noise8").to_vec())), &limits).map(|_| ());
    assert!(malformed(got).contains("limit"));
}

#[test]
fn variable_block_size_frames_take_their_start_from_the_coded_number() {
    let (_, packets) = raw_packets(oga("stereo16"));
    let mut frame = Vec::new();
    let header = FrameHeader {
        variable_block_size: true,
        block_size: 256,
        sample_rate: None,
        channels: ChannelAssignment::Independent(2),
        bits_per_sample: None,
        number: 123_456,
        header_len: 0,
    };
    frame::write_header(&header, &mut frame);
    frame.extend([0; 8]);
    let all = [packets[..2].to_vec(), vec![frame]].concat();
    let pages = testutil::pages(1, 0, &all, &[1, 255]);
    let mut d = open_bytes(testutil::bytes(&pages)).unwrap();
    let got = read_all(d.as_mut()).unwrap();
    assert_eq!((got[0].pts, got[0].duration), (123_456, 256));
}

#[test]
fn a_stream_with_no_audio_is_empty() {
    let (_, packets) = raw_packets(oga("stereo16"));
    let pages = testutil::pages(1, 0, &packets[..2], &[1, 255]);
    let mut d = open_bytes(testutil::bytes(&pages)).unwrap();
    assert_eq!(read_all(d.as_mut()).unwrap(), []);
}

#[test]
fn a_stream_with_only_its_first_page_is_empty() {
    let pages = stream_pages("stereo16", 1, &[255]);
    let mut d = open_bytes(pages[0].to_bytes()).unwrap();
    assert_eq!(read_all(d.as_mut()).unwrap(), []);
}
