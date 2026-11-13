//! Seek-then-decode equals decode-then-slice, bit for bit, through `DecodeSession`, on Ogg
//! Vorbis streams that mix both block sizes, with priming, a late start and end padding.

use crossfade_core::{AudioFrame, Limits, Samples};
use crossfade_graph::DecodeSession;
use crossfade_testkit::memory::MemoryIo;
use crossfade_vorbis::testing::{self as w, OggLayout};
use proptest::prelude::*;

fn open(bytes: &[u8]) -> DecodeSession {
    let mut io = MemoryIo::new();
    io.insert("in.ogg", bytes.to_vec());
    DecodeSession::open("in.ogg", &mut io, &Limits::default()).unwrap()
}

/// Every sample from the session's position on, as bits, checking the frames follow on.
fn rest(s: &mut DecodeSession) -> Vec<u32> {
    let mut out = vec![];
    let mut f = AudioFrame::new(&s.stream().audio);
    let mut pts = s.position();
    while s.next_frame(&mut f).unwrap() {
        assert_eq!(f.pts as u64, pts, "frames follow on");
        pts += f.frames() as u64;
        let Samples::F32(v) = &f.samples else {
            panic!("vorbis decodes to f32")
        };
        out.extend(v.iter().map(|x| x.to_bits()));
    }
    out
}

fn file(seq: &str, seed: u64, layout: &OggLayout) -> Vec<u8> {
    let s = w::sample_stream(2, seq, seed);
    w::ogg_file(
        [&s.headers[0], &s.headers[1], &s.headers[2]],
        &s.audio,
        layout,
    )
}

const SEQ: &str = "SSLLSLSSSSLLLSLSSLSLLLSSLSSSSLLS";

fn layouts() -> [OggLayout; 3] {
    [
        OggLayout::default(),
        OggLayout {
            packets_per_page: vec![1],
            start: -1200,
            end_trim: 333,
            ..OggLayout::default()
        },
        OggLayout {
            packets_per_page: vec![5, 2],
            start: 7000,
            end_trim: 10,
            ..OggLayout::default()
        },
    ]
}

fn check(bytes: &[u8], targets: &[u64]) {
    let all = rest(&mut open(bytes));
    let total = (all.len() / 2) as u64;
    assert_eq!(open(bytes).stream().frames, Some(total));
    let mut s = open(bytes);
    for &t in targets {
        s.seek(t).unwrap();
        let from = t.min(total) as usize * 2;
        assert!(rest(&mut s) == all[from..], "target {t} of {total}");
    }
}

#[test]
fn every_kind_of_target_in_every_layout() {
    for layout in layouts() {
        let bytes = file(SEQ, 5, &layout);
        let total = rest(&mut open(&bytes)).len() as u64 / 2;
        let mut targets = vec![
            0,
            1,
            127,
            128,
            1023,
            1024,
            total / 2,
            total - 1,
            total,
            total + 9,
        ];
        targets.extend((0..total).step_by(997));
        check(&bytes, &targets);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn random_targets_on_random_streams(
        seq in "[SL]{4,40}",
        seed in any::<u64>(),
        layout in 0usize..3,
        targets in prop::collection::vec(any::<u64>(), 1..8),
    ) {
        let bytes = file(&seq, seed, &layouts()[layout]);
        let total = rest(&mut open(&bytes)).len() as u64 / 2;
        let targets: Vec<u64> = targets.iter().map(|t| t % (total + 2)).collect();
        check(&bytes, &targets);
    }
}
