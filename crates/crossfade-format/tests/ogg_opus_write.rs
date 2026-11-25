//! The Ogg Opus writer (RFC 7845): files made with `crossfade_opus::testing::ogg` are read,
//! written again and read back, and the pages the writer produced are checked on their own.

mod seek_support;

use crossfade_core::{Demuxer, Limits, Packet, StreamInfo, Tags};
use crossfade_format::{FormatId, create_muxer, open_demuxer};
use crossfade_opus::testing::ogg::{self as w, OggLayout};
use seek_support::Shared;
use std::io::Cursor;

fn open(bytes: &[u8]) -> Box<dyn Demuxer> {
    open_demuxer(Box::new(Cursor::new(bytes.to_vec())), &Limits::default())
        .unwrap()
        .1
}

fn all(d: &mut dyn Demuxer) -> Vec<Packet> {
    let mut out = vec![];
    let mut p = Packet::default();
    while d.read_packet(&mut p).unwrap() {
        out.push(p.clone());
    }
    out
}

struct Read {
    info: StreamInfo,
    tags: Tags,
    packets: Vec<Packet>,
}

fn read(bytes: &[u8]) -> Read {
    let mut d = open(bytes);
    Read {
        info: d.streams()[0].clone(),
        tags: d.tags().clone(),
        packets: all(d.as_mut()),
    }
}

/// Stream copy: what a reader hands out goes to the writer unchanged.
fn copy(bytes: &[u8]) -> Vec<u8> {
    let mut d = open(bytes);
    let sink = Shared::default();
    let mut m = create_muxer(
        FormatId::Ogg,
        Box::new(sink.clone()),
        &d.streams()[0],
        d.tags(),
    )
    .unwrap();
    for p in all(d.as_mut()) {
        m.write_packet(&p).unwrap();
    }
    m.finish().unwrap();
    sink.bytes()
}

fn audio() -> Vec<Vec<u8>> {
    let mut a = vec![];
    for (i, &(config, n)) in [(9, 3), (3, 2), (15, 4), (16, 3), (29, 2), (31, 5), (1, 2)]
        .iter()
        .enumerate()
    {
        a.extend(w::packets(config, true, n, 40, i as u64));
    }
    a
}

const TAGS: [&str; 3] = ["TITLE=Tone", "ARTIST=Nobody", "R128_TRACK_GAIN=-1024"];

fn file(pre_skip: u16, audio: &[Vec<u8>], layout: &OggLayout) -> Vec<u8> {
    w::ogg_file(
        &w::opus_head(2, pre_skip, 0, None),
        &w::opus_tags("source", &TAGS),
        audio,
        layout,
    )
}

fn assert_same(a: &Read, b: &Read) {
    assert_eq!(a.info, b.info);
    assert_eq!(a.tags, b.tags);
    assert_eq!(a.packets, b.packets);
}

#[test]
fn a_stream_copy_reads_back_the_same() {
    let src = file(312, &audio(), &OggLayout::default());
    let written = copy(&src);
    assert_same(&read(&src), &read(&written));
}
