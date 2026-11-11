//! The threaded decoder gives what the one-frame-at-a-time decoder gives: the same blocks in
//! the same order, the same verification, and on a broken stream the same error at the same
//! frame, with every block before it delivered and none after.
mod common;

use common::*;
use crossfade_flac::streaminfo::StreamInfoBlock;
use crossfade_flac::{Decoder, EncoderConfig, Error, Verification};
use proptest::prelude::*;

/// What a caller sees: blocks (first sample, length, interleaved samples), then possibly an
/// error, then what `finish` says.
#[derive(Debug, PartialEq)]
struct Seen {
    blocks: Vec<(u64, usize, Vec<i32>)>,
    error: Option<Error>,
    end: std::result::Result<Verification, Error>,
}

/// The reference: `decode_frame` one frame after another, stopping at the first error.
fn reference(info: &StreamInfoBlock, frames: &[Vec<u8>]) -> Seen {
    let mut dec = Decoder::new(info).unwrap();
    let (mut blocks, mut error) = (Vec::new(), None);
    for f in frames {
        match dec.decode_frame(f) {
            Ok(b) => {
                let mut v = Vec::new();
                b.interleave_i32(0, &mut v);
                blocks.push((b.first_sample(), b.len(), v));
            }
            Err(e) => {
                error = Some(e);
                break;
            }
        }
    }
    Seen {
        blocks,
        error,
        end: dec.finish(),
    }
}

/// The push interface on `threads` threads; `flush_every` (0: never) flushes after that many
/// frames as well as at the end.
fn pushed(info: &StreamInfoBlock, frames: &[Vec<u8>], threads: usize, flush_every: usize) -> Seen {
    let mut dec = Decoder::with_threads(info, threads).unwrap();
    let mut blocks = Vec::new();
    let mut take = |b: crossfade_flac::Block<'_>| {
        let mut v = Vec::new();
        b.interleave_i32(0, &mut v);
        blocks.push((b.first_sample(), b.len(), v));
    };
    let mut error = None;
    for (i, f) in frames.iter().enumerate() {
        let mut r = dec.push_frame(f, &mut take);
        if r.is_ok() && flush_every > 0 && (i + 1) % flush_every == 0 {
            r = dec.flush(&mut take);
        }
        if let Err(e) = r {
            error = Some(e);
            break;
        }
    }
    if error.is_none()
        && let Err(e) = dec.flush(&mut take)
    {
        error = Some(e);
    }
    assert_eq!(dec.frames_in_flight(), 0);
    if let Some(e) = &error {
        // The stream is over: the same error again, and nothing more delivered.
        let before = blocks.len();
        assert_eq!(dec.push_frame(&frames[0], &mut take).as_ref(), Err(e));
        assert_eq!(dec.flush(&mut take).as_ref(), Err(e));
        assert_eq!(blocks.len(), before);
    }
    Seen {
        blocks,
        error,
        end: dec.finish(),
    }
}

fn frames_of(e: &Encoded) -> Vec<Vec<u8>> {
    e.frames
        .iter()
        .map(|&(s, t)| e.bytes[s..t].to_vec())
        .collect()
}

fn stream(
    bits: u8,
    channels: u8,
    level: u8,
    kind: u8,
    frames: usize,
    block: usize,
    seed: u64,
) -> Encoded {
    let config = EncoderConfig {
        sample_rate: 44_100,
        channels,
        bits_per_sample: bits,
        level,
        threads: 1,
    };
    let ch = usize::from(channels);
    let samples = signal(kind, bits, frames * ch, ch, seed);
    encode(config, &samples, block)
}

/// Ways to break a stream, each applied at frame `at` (taken modulo the frame count).
fn corrupt(frames: &mut Vec<Vec<u8>>, how: u8, at: usize, seed: u64) {
    if frames.is_empty() {
        return;
    }
    let k = at % frames.len();
    let mut rng = Rng(seed | 1);
    match how % 6 {
        // A flipped bit anywhere in the frame (header, subframes or CRC).
        0 => {
            let f = &mut frames[k];
            let bit = (rng.next() % (f.len() as u64 * 8)) as usize;
            f[bit / 8] ^= 1 << (bit % 8);
        }
        // Cut short.
        1 => {
            let f = &mut frames[k];
            let keep = (rng.next() % f.len() as u64) as usize;
            f.truncate(keep);
        }
        // Two frames swapped: each decodes, but numbers and the MD5 disagree.
        2 if frames.len() > 1 => {
            let j = (k + 1) % frames.len();
            frames.swap(k, j);
        }
        // The head of one frame on the tail of the next.
        3 if frames.len() > 1 => {
            let j = (k + 1) % frames.len();
            let half = frames[k].len() / 2;
            let mut spliced = frames[k][..half].to_vec();
            spliced.extend_from_slice(&frames[j][frames[j].len() / 2..]);
            frames[k] = spliced;
        }
        // A frame repeated: the stream gets longer than STREAMINFO says.
        4 => {
            let copy = frames[k].clone();
            frames.insert(k, copy);
        }
        // Several bits flipped in several frames.
        _ => {
            for _ in 0..3 {
                let i = (rng.next() % frames.len() as u64) as usize;
                let f = &mut frames[i];
                let bit = (rng.next() % (f.len() as u64 * 8)) as usize;
                f[bit / 8] ^= 1 << (bit % 8);
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn every_thread_count_decodes_a_valid_stream_the_same(
        bits in 4u8..=32,
        channels in 1u8..=8,
        level in 0u8..=8,
        kind in 0u8..KINDS,
        count in 0usize..40,
        block in 16usize..300,
        seed in any::<u64>(),
        flush_every in 0usize..7,
    ) {
        let e = stream(bits, channels, level, kind, count * block + block / 3, block, seed);
        let frames = frames_of(&e);
        let want = reference(&e.info, &frames);
        prop_assert_eq!(&want.error, &None);
        prop_assert_eq!(&want.end, &Ok(Verification::Md5Matched));
        for threads in [1, 2, 3, 8] {
            prop_assert_eq!(&pushed(&e.info, &frames, threads, flush_every), &want, "{} threads", threads);
        }
    }

    #[test]
    fn every_thread_count_fails_a_broken_stream_at_the_same_frame(
        bits in 4u8..=32,
        channels in 1u8..=4,
        level in 0u8..=8,
        kind in 0u8..KINDS,
        count in 1usize..30,
        block in 16usize..200,
        seed in any::<u64>(),
        how in any::<u8>(),
        at in any::<usize>(),
        flush_every in 0usize..5,
    ) {
        let e = stream(bits, channels, level, kind, count * block + 5, block, seed);
        let mut frames = frames_of(&e);
        corrupt(&mut frames, how, at, seed);
        let want = reference(&e.info, &frames);
        // A failed stream never reports a match.
        if want.error.is_some() {
            prop_assert_eq!(&want.end, &Ok(Verification::Skipped));
        }
        for threads in [1, 2, 3, 8] {
            prop_assert_eq!(&pushed(&e.info, &frames, threads, flush_every), &want, "{} threads", threads);
        }
    }
}

#[test]
fn auto_threads_decode_the_same() {
    let e = stream(16, 2, 5, 7, 4096 * 30 + 77, 4096, 3);
    let frames = frames_of(&e);
    let want = reference(&e.info, &frames);
    assert_eq!(pushed(&e.info, &frames, 0, 0), want);
    assert_eq!(pushed(&e.info, &frames, Decoder::AUTO_THREADS, 5), want);
}

#[test]
fn frames_in_flight_are_bounded_and_flushed() {
    let e = stream(16, 2, 0, 7, 1152 * 60, 1152, 4);
    for threads in [1, 4] {
        let mut dec = Decoder::with_threads(&e.info, threads).unwrap();
        let mut n = 0;
        for (i, &(s, t)) in e.frames.iter().enumerate() {
            dec.push_frame(&e.bytes[s..t], &mut |_| n += 1).unwrap();
            assert!(dec.frames_in_flight() <= 2 * threads, "{threads} threads");
            if threads == 1 {
                assert_eq!(dec.frames_in_flight(), 0);
            }
            assert_eq!(n + dec.frames_in_flight(), i + 1);
        }
        dec.flush(&mut |_| n += 1).unwrap();
        assert_eq!((n, dec.frames_in_flight()), (60, 0));
        assert_eq!(dec.finish(), Ok(Verification::Md5Matched));
    }
}

/// `finish` refuses while frames are in flight (their blocks would be lost and the totals
/// would not cover them); after a flush it verifies as usual.
#[test]
fn finish_waits_for_a_flush() {
    let e = stream(24, 2, 5, 8, 4096 * 20, 4096, 5);
    let mut dec = Decoder::with_threads(&e.info, 4).unwrap();
    let mut n = 0;
    for &(s, t) in &e.frames {
        dec.push_frame(&e.bytes[s..t], &mut |_| n += 1).unwrap();
    }
    if dec.frames_in_flight() > 0 {
        assert!(matches!(dec.finish(), Err(Error::Malformed(_))));
    }
    dec.flush(&mut |_| n += 1).unwrap();
    assert_eq!(n, 20);
    assert_eq!(dec.finish(), Ok(Verification::Md5Matched));
    assert_eq!(dec.finish(), Ok(Verification::Skipped));
}

#[test]
fn decode_frame_waits_for_a_flush_too() {
    let e = stream(16, 2, 5, 7, 4096 * 12, 4096, 6);
    let mut dec = Decoder::with_threads(&e.info, 4).unwrap();
    let (s, t) = e.frames[0];
    for &(s, t) in &e.frames[..11] {
        dec.push_frame(&e.bytes[s..t], &mut |_| {}).unwrap();
    }
    if dec.frames_in_flight() > 0 {
        assert!(matches!(
            dec.decode_frame(&e.bytes[s..t]),
            Err(Error::Malformed(_))
        ));
    }
}

#[test]
fn nothing_pushed_and_empty_flushes_are_fine() {
    let e = stream(16, 2, 5, 5, 0, 4096, 7);
    for threads in [1, 3] {
        let mut dec = Decoder::with_threads(&e.info, threads).unwrap();
        dec.flush(&mut |_| panic!("no block")).unwrap();
        dec.flush(&mut |_| panic!("no block")).unwrap();
        assert_eq!(dec.finish(), Ok(Verification::Md5Matched));
    }
}

#[test]
fn dropping_a_busy_decoder_returns() {
    let e = stream(24, 2, 8, 8, 4096 * 30, 4096, 8);
    for threads in [2, 8] {
        let mut dec = Decoder::with_threads(&e.info, threads).unwrap();
        for &(s, t) in &e.frames {
            dec.push_frame(&e.bytes[s..t], &mut |_| {}).unwrap();
        }
        drop(dec);
    }
}
