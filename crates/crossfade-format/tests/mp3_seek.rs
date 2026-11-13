//! Seeking an MP3 file is exact for the caller: seek, reset the decoder, decode and drop what
//! lies before the target, and the samples are bit for bit those of a decode from the start,
//! for every kind of file the fixtures hold, the lowest bitrate of each MPEG version (whose
//! small frames need the most pre-roll for the bit reservoir) included.

use crossfade_codec::decoder_for;
use crossfade_core::{AudioFrame, Decoder, Demuxer, Limits, Packet, Samples};
use crossfade_format::open_demuxer;
use std::io::Cursor;

fn open(name: &str) -> (Box<dyn Demuxer>, Box<dyn Decoder>) {
    let path = format!(
        "{}/tests/fixtures/mp3/{name}.mp3",
        env!("CARGO_MANIFEST_DIR")
    );
    let bytes = std::fs::read(path).unwrap();
    let (_, d) = open_demuxer(Box::new(Cursor::new(bytes)), &Limits::default()).unwrap();
    let dec = decoder_for(&d.streams()[0]).unwrap();
    (d, dec)
}

/// (pts, interleaved frame) for everything decoded from where the reader stands.
fn decode_rest(d: &mut dyn Demuxer, dec: &mut dyn Decoder) -> Vec<(i64, Vec<f32>)> {
    let ch = usize::from(d.streams()[0].audio.layout().channels());
    let mut out = Vec::new();
    let mut p = Packet::default();
    let mut f = AudioFrame::new(&d.streams()[0].audio);
    while d.read_packet(&mut p).unwrap() {
        dec.send_packet(&p).unwrap();
        while dec.receive_frame(&mut f).unwrap() {
            let Samples::F32(s) = &f.samples else {
                unreachable!()
            };
            out.extend(
                s.chunks(ch)
                    .enumerate()
                    .map(|(i, x)| (f.pts + i as i64, x.to_vec())),
            );
        }
    }
    out
}

/// The caller's side of a seek: seek, reset, decode, drop what lies before `target`.
fn seek_and_decode(
    d: &mut dyn Demuxer,
    dec: &mut dyn Decoder,
    target: i64,
) -> (i64, Vec<(i64, Vec<f32>)>) {
    let landed = d.seek(target).unwrap();
    dec.reset();
    let rest = decode_rest(d, dec);
    if let Some((first, _)) = rest.first() {
        assert_eq!(*first, landed, "the reader said where it landed");
    }
    (
        landed,
        rest.into_iter().filter(|(pts, _)| *pts >= target).collect(),
    )
}

struct Rng(u64);
impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

#[test]
fn seek_then_decode_equals_decode_then_slice() {
    for name in [
        "cbr320",
        "mpeg1-32k",
        "mpeg2",
        "mpeg2-8k",
        "mpeg25",
        "vbr",
        "free",
        "notag",
    ] {
        let (mut d, mut dec) = open(name);
        let whole = decode_rest(&mut *d, &mut *dec);
        let first = whole[0].0;
        let end = d.streams()[0]
            .frames
            .map_or(whole.last().unwrap().0 + 1, |f| f as i64);
        let span = (end - first) as u64;
        let mut rng = Rng(0x5EED ^ span);
        let mut targets: Vec<i64> = vec![first, 0, end - 1, end, end + 10_000, i64::MIN, -1, 1];
        targets.extend((0..12).map(|_| first + rng.below(span) as i64));
        // Repeated and backwards.
        targets.extend([targets[10], targets[10], targets[9], 3]);
        for target in targets {
            let (landed, got) = seek_and_decode(&mut *d, &mut *dec, target);
            assert!(
                landed <= target.max(first),
                "{name} {target}: landed at {landed}"
            );
            if target >= end {
                assert_eq!(landed, end, "{name} {target}");
                assert!(got.is_empty(), "{name} {target}");
                continue;
            }
            let want: Vec<_> = whole
                .iter()
                .filter(|(pts, _)| *pts >= target)
                .cloned()
                .collect();
            assert_eq!(got.len(), want.len(), "{name} {target}");
            for (g, w) in got.iter().zip(&want) {
                assert_eq!(g.0, w.0, "{name} {target}");
                let same =
                    g.1.iter()
                        .zip(&w.1)
                        .all(|(a, b)| a.to_bits() == b.to_bits());
                assert!(same, "{name}: seek to {target} differs at pts {}", g.0);
            }
        }
    }
}

#[test]
fn without_the_pre_roll_the_samples_at_the_target_differ() {
    // What the pre-roll buys: decoding from the target's own frame (no overlap, no history,
    // no reservoir) gives other samples there.
    let (mut d, mut dec) = open("mpeg1-32k");
    let whole = decode_rest(&mut *d, &mut *dec);
    let target = whole[whole.len() / 2].0;
    let landed = d.seek(target).unwrap();
    dec.reset();
    let mut p = Packet::default();
    let mut f = AudioFrame::new(&d.streams()[0].audio);
    let mut got = None;
    while d.read_packet(&mut p).unwrap() {
        if p.pts + 1152 <= target {
            continue;
        }
        dec.send_packet(&p).unwrap();
        assert!(dec.receive_frame(&mut f).unwrap());
        let Samples::F32(s) = &f.samples else {
            unreachable!()
        };
        got = Some(s[(target - p.pts) as usize]);
        break;
    }
    assert!(landed < target - 1152);
    let want = whole.iter().find(|(pts, _)| *pts == target).unwrap().1[0];
    assert_ne!(got.unwrap().to_bits(), want.to_bits());
}
