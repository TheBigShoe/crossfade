#![no_main]
//! The Ogg Opus writer's round trip: a valid Ogg Opus file (any mode and frame size, mono to
//! multistream, pre-skip, a cut front, end trimming, pages of any size) copied through the
//! writer reads back with the same packets, timeline and length. Panics by design when that
//! fails.

use arbitrary::{Arbitrary, Unstructured};
use crossfade_core::{Demuxer, Packet, StreamInfo};
use crossfade_format::{FormatId, create_muxer};
use crossfade_fuzz::{SharedSink, limits};
use crossfade_opus::testing::ogg::{self as w, OggLayout};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fn read(d: &mut dyn Demuxer) -> (StreamInfo, Vec<Packet>) {
    let mut packets = vec![];
    let mut p = Packet::default();
    while d.read_packet(&mut p).expect("a valid file reads to its end") {
        packets.push(p.clone());
    }
    (d.streams()[0].clone(), packets)
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let (Ok(multi), Ok(configs), Ok(seed), Ok(per_page), Ok(start), Ok(trim), Ok(pre_skip)) = (
        bool::arbitrary(&mut u),
        Vec::<u8>::arbitrary(&mut u),
        u64::arbitrary(&mut u),
        u.int_in_range(1usize..=9),
        u.int_in_range(0i64..=100_000),
        u.int_in_range(0i64..=2000),
        u.int_in_range(0u16..=8000),
    ) else {
        return;
    };
    let mut audio = vec![];
    for (i, c) in configs.iter().take(400).enumerate() {
        let config = c & 31;
        let stereo = c & 0x20 != 0;
        let len = usize::from(c >> 6) * 30 + 1;
        let mut p = w::packets(config, stereo, 1, len, seed.wrapping_add(i as u64));
        if multi {
            let mono = w::packets(config, false, 1, 20, seed.wrapping_add(1000 + i as u64));
            p = vec![w::multistream_packet(&[p[0].clone(), mono[0].clone()])];
        }
        audio.extend(p);
    }
    let head = if multi {
        w::opus_head(3, pre_skip, 0, Some((1, 2, 1, &[0, 2, 1])))
    } else {
        w::opus_head(2, pre_skip, -256, None)
    };
    let layout = OggLayout {
        packets_per_page: vec![per_page],
        start,
        end_trim: trim,
        ..OggLayout::default()
    };
    let bytes = w::ogg_file(&head, &w::opus_tags("fuzz", &["TITLE=x"]), &audio, &layout);
    // A trim past the only page's audio is a file the reader refuses.
    let Ok(mut source) = crossfade_format::ogg::open(Box::new(Cursor::new(bytes)), &limits())
    else {
        return;
    };
    let stream = source.streams()[0].clone();
    let tags = source.tags().clone();
    let want = read(&mut *source);

    let sink = SharedSink::default();
    let mut m = create_muxer(FormatId::Ogg, Box::new(sink.clone()), &stream, &tags)
        .expect("the reader's stream can be written");
    for p in &want.1 {
        m.write_packet(p).expect("a memory sink takes every packet");
    }
    m.finish().expect("a memory sink finishes");

    let mut copy = crossfade_format::ogg::open(Box::new(Cursor::new(sink.bytes())), &limits())
        .expect("the copy opens");
    assert_eq!(copy.tags(), &tags);
    let got = read(&mut *copy);
    assert_eq!(got.0, want.0, "stream");
    assert!(got.1 == want.1, "packets");
});
