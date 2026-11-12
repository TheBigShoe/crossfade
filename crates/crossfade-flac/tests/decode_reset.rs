//! `reset` after a seek: whatever was in flight or failed is forgotten, the frames that follow
//! decode as a fresh decoder decodes them, at any thread count, and the stream's MD5 and sample
//! total are no longer checked.
mod common;

use common::*;
use crossfade_flac::{Decoder, EncoderConfig, Verification};

type Blocks = Vec<(u64, usize, Vec<i32>)>;

fn encoded(frames: usize) -> Encoded {
    let config = EncoderConfig {
        sample_rate: 44_100,
        channels: 2,
        bits_per_sample: 16,
        level: 5,
        threads: 1,
    };
    let block = 1152;
    encode(config, &signal(1, 16, frames * block * 2, 2, 7), block)
}

fn frames_of(e: &Encoded) -> Vec<Vec<u8>> {
    e.frames
        .iter()
        .map(|&(s, t)| e.bytes[s..t].to_vec())
        .collect()
}

fn push_all(dec: &mut Decoder, frames: &[Vec<u8>], out: &mut Blocks) {
    let mut take = |b: crossfade_flac::Block<'_>| {
        let mut v = Vec::new();
        b.interleave_i32(0, &mut v);
        out.push((b.first_sample(), b.len(), v));
    };
    for f in frames {
        dec.push_frame(f, &mut take).unwrap();
    }
    dec.flush(&mut take).unwrap();
}

fn fresh(e: &Encoded, frames: &[Vec<u8>]) -> Blocks {
    let mut dec = Decoder::new(&e.info).unwrap();
    let mut out = Vec::new();
    push_all(&mut dec, frames, &mut out);
    out
}

#[test]
fn after_a_reset_the_next_frames_decode_as_a_fresh_decoder_decodes_them() {
    let e = encoded(24);
    let frames = frames_of(&e);
    let want = fresh(&e, &frames[15..]);
    for threads in [1, 2, 3, 8] {
        let mut dec = Decoder::with_threads(&e.info, threads).unwrap();
        // Ten frames pushed and never flushed: with threads some are still in flight.
        for f in &frames[..10] {
            dec.push_frame(f, &mut |_| {}).unwrap();
        }
        dec.reset();
        assert_eq!(dec.frames_in_flight(), 0, "{threads} threads");
        let mut got = Vec::new();
        push_all(&mut dec, &frames[15..], &mut got);
        assert_eq!(got, want, "{threads} threads");
        assert_eq!(dec.finish(), Ok(Verification::Skipped), "{threads} threads");
    }
}

#[test]
fn a_reset_clears_a_failure() {
    let e = encoded(12);
    let frames = frames_of(&e);
    let want = fresh(&e, &frames[6..]);
    for threads in [1, 3] {
        let mut dec = Decoder::with_threads(&e.info, threads).unwrap();
        let mut broken = frames[2].clone();
        broken.truncate(broken.len() / 2);
        let mut failed = None;
        for f in [&frames[0], &frames[1], &broken, &frames[3]] {
            if let Err(e) = dec.push_frame(f, &mut |_| {}) {
                failed = Some(e);
                break;
            }
        }
        if failed.is_none() {
            failed = dec.flush(&mut |_| {}).err();
        }
        assert!(failed.is_some(), "{threads} threads");
        dec.reset();
        let mut got = Vec::new();
        push_all(&mut dec, &frames[6..], &mut got);
        assert_eq!(got, want, "{threads} threads");
        assert_eq!(dec.finish(), Ok(Verification::Skipped));
    }
}

#[test]
fn a_reset_at_the_start_still_turns_verification_off() {
    let e = encoded(3);
    let frames = frames_of(&e);
    let mut dec = Decoder::new(&e.info).unwrap();
    dec.reset();
    push_all(&mut dec, &frames, &mut Vec::new());
    // The whole stream went through, but after a reset nothing says it started at the start.
    assert_eq!(dec.finish(), Ok(Verification::Skipped));
}
