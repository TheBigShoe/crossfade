//! Seeking an Ogg Opus stream through `DecodeSession`. Opus is the one codec where
//! seek-then-decode is not decode-then-slice bit for bit: the decoder restarts 1 s before the
//! target (RFC 7845 4.6 asks for at least 80 ms) and has converged closely, not exactly, by
//! then (the tier-3 `conformance_opus` measures how closely on real audio). What holds exactly, and is tested
//! here: the session lands where the reader says, decodes from there as a fresh decoder does,
//! drops what lies before the target, and its frames follow on to the end.

use crossfade_core::{AudioFrame, Limits, Samples};
use crossfade_graph::DecodeSession;
use crossfade_opus::testing::ogg::{self as w, OggLayout};
use crossfade_testkit::memory::MemoryIo;

const PRE_SKIP: i64 = 312;

fn open(bytes: &[u8]) -> DecodeSession {
    let mut io = MemoryIo::new();
    io.insert("in.opus", bytes.to_vec());
    DecodeSession::open("in.opus", &mut io, &Limits::default()).unwrap()
}

/// Every sample from the session's position on, checking the frames follow on.
fn rest(s: &mut DecodeSession) -> Vec<f32> {
    let mut out = vec![];
    let mut f = AudioFrame::new(&s.stream().audio);
    let mut pts = s.position();
    while s.next_frame(&mut f).unwrap() {
        assert_eq!(f.pts as u64, pts, "frames follow on");
        pts += f.frames() as u64;
        let Samples::F32(v) = &f.samples else {
            panic!("opus decodes to f32")
        };
        out.extend_from_slice(v);
    }
    out
}

/// SILK, hybrid and CELT runs, one packet a page.
fn audio() -> Vec<Vec<u8>> {
    let mut a = vec![];
    for (i, config) in [9u8, 1, 15, 13, 31, 29, 9, 15, 31].into_iter().enumerate() {
        a.extend(w::packets(config, true, 36, 50, i as u64 * 100));
    }
    a
}

#[test]
fn a_seek_decodes_as_a_fresh_decoder_from_where_the_reader_lands() {
    let a = audio();
    let durations: Vec<i64> = a
        .iter()
        .map(|p| crossfade_opus::packet_duration(p).unwrap() as i64)
        .collect();
    let bytes = w::ogg_file(
        &w::opus_head(2, PRE_SKIP as u16, 0, None),
        &w::opus_tags("t", &[]),
        &a,
        &OggLayout {
            packets_per_page: vec![1],
            end_trim: 500,
            ..OggLayout::default()
        },
    );
    let whole = rest(&mut open(&bytes));
    let total = (whole.len() / 2) as u64;
    assert_eq!(open(&bytes).stream().frames, Some(total));
    let mut s = open(&bytes);
    let mut worst = 0.0f32;
    for t in [
        0u64,
        1,
        3839,
        48_000,
        60_000,
        150_000,
        3841,
        10_000,
        20_000,
        total / 2,
        total - 1,
        total,
        total + 7,
    ] {
        s.seek(t).unwrap();
        let got = rest(&mut s);
        let from = t.min(total) as usize;
        assert_eq!(got.len(), whole.len() - from * 2, "target {t}");
        if t >= total {
            continue;
        }
        // The landing: the packet after the last page (here, packet) ending 1 s or more
        // before the target's granule.
        let goal = t as i64 + PRE_SKIP - 48_000;
        let (mut k, mut end) = (0, 0);
        while k < a.len() && end + durations[k] <= goal {
            end += durations[k];
            k += 1;
        }
        let mut d = crossfade_opus::Decoder::new(2).unwrap();
        let mut fresh = vec![];
        for p in &a[k..] {
            d.decode(Some(p), &mut fresh).unwrap();
        }
        let skip = (t as i64 + PRE_SKIP - end) as usize * 2;
        assert!(got == fresh[skip..skip + got.len()], "target {t}");
        for (x, y) in got.iter().zip(&whole[from * 2..]) {
            worst = worst.max((x - y).abs());
        }
    }
    println!("worst difference from decode-then-slice: {worst:e}");
}
