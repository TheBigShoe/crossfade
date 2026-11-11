//! The thread count changes nothing in the output, and a threaded encoder starts, stops and
//! fails cleanly. (Termination is the assertion where a test drops an encoder mid-stream.)
mod common;

use common::*;
use crossfade_flac::{Decoder, Encoder, EncoderConfig, Error, Verification};
use proptest::prelude::*;

fn cfg(channels: u8, bits: u8, level: u8, threads: usize) -> EncoderConfig {
    EncoderConfig {
        sample_rate: 44_100,
        channels,
        bits_per_sample: bits,
        level,
        threads,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn every_thread_count_writes_the_same_stream(
        bits in 4u8..=32,
        channels in 1u8..=8,
        level in 0u8..=8,
        kind in 0u8..KINDS,
        blocks in 0usize..6,
        extra in any::<u16>(),
        seed in any::<u64>(),
        chunking in any::<u64>(),
    ) {
        let one = cfg(channels, bits, level, 1);
        let block = Encoder::new(one).unwrap().block_size();
        // Many channels get fewer blocks, to keep a case cheap.
        let blocks = if channels > 2 { blocks.min(3) } else { blocks };
        let frames = blocks * block + usize::from(extra) % block;
        let ch = usize::from(channels);
        let samples = signal(kind, bits, frames * ch, ch, seed);
        let want = written(one, &samples, chunking);
        prop_assert_eq!(want.ends.len(), frames.div_ceil(block));
        for threads in [2, 3, 8] {
            let got = written(cfg(channels, bits, level, threads), &samples, chunking);
            prop_assert_eq!(&got, &want, "{} threads", threads);
        }
        // And the same as one encode_block call per block.
        let exact = encode(one, &samples, 0);
        prop_assert_eq!(&want.bytes, &exact.bytes);
        prop_assert_eq!(want.info, exact.info);
    }
}

#[test]
fn auto_threads_write_the_same_stream() {
    let samples = signal(7, 16, 4096 * 2 * 9 + 300, 2, 3);
    let want = written(cfg(2, 16, 5, 1), &samples, 9);
    assert_eq!(written(cfg(2, 16, 5, 0), &samples, 9), want);
    assert_eq!(
        written(cfg(2, 16, 5, EncoderConfig::AUTO_THREADS), &samples, 0),
        want
    );
}

#[test]
fn new_codes_on_the_calling_thread() {
    assert_eq!(EncoderConfig::new(44_100, 2, 16).threads, 1);
    assert_eq!(EncoderConfig::AUTO_THREADS, 0);
}

#[test]
fn frames_in_flight_come_out_by_the_flush() {
    let samples = signal(7, 16, 1152 * 2 * 40, 2, 4);
    for threads in [1, 4] {
        let mut enc = Encoder::new(cfg(2, 16, 0, threads)).unwrap();
        let (mut bytes, mut ends) = (Vec::new(), Vec::new());
        enc.write_ends(&samples, &mut bytes, &mut ends).unwrap();
        assert_eq!(ends.len() + enc.frames_in_flight(), 40, "{threads} threads");
        assert!(enc.frames_in_flight() <= 2 * threads);
        if threads == 1 {
            assert_eq!(enc.frames_in_flight(), 0);
        }
        enc.flush_ends(&mut bytes, &mut ends).unwrap();
        assert_eq!((ends.len(), enc.frames_in_flight()), (40, 0));
        assert_eq!(ends.last(), Some(&bytes.len()));
    }
}

#[test]
fn flush_with_nothing_buffered_writes_nothing() {
    for threads in [1, 4] {
        let mut enc = Encoder::new(cfg(2, 16, 5, threads)).unwrap();
        let mut out = Vec::new();
        enc.flush(&mut out).unwrap();
        assert!(out.is_empty());
        enc.write(&signal(7, 16, 4096 * 2 * 3, 2, 5), &mut out)
            .unwrap();
        enc.flush(&mut out).unwrap();
        let len = out.len();
        enc.flush(&mut out).unwrap();
        assert_eq!(out.len(), len, "{threads} threads");
    }
}

#[test]
fn zero_samples_and_finishing_twice_are_the_same_at_any_thread_count() {
    let one = written(cfg(2, 16, 5, 1), &[], 0);
    assert!(one.bytes.is_empty() && one.ends.is_empty());
    assert_eq!(one.info.total_samples, 0);
    for threads in [2, 8] {
        let mut enc = Encoder::new(cfg(2, 16, 5, threads)).unwrap();
        let mut out = Vec::new();
        enc.write(&[], &mut out).unwrap();
        enc.flush(&mut out).unwrap();
        let info = enc.finish();
        assert_eq!(info, one.info);
        assert_eq!(enc.finish(), info);
        assert!(out.is_empty());
        assert!(matches!(
            enc.write(&[1, 2], &mut out),
            Err(Error::Malformed(_))
        ));
    }
}

/// `finish` without `flush` leaves out what was not written yet, frames in flight included, and
/// the STREAMINFO's total and frame sizes describe exactly the frames that were. The MD5 is
/// taken as blocks go to the workers, so when frames in flight are dropped it is left unknown
/// (zero) rather than wrong.
#[test]
fn finish_without_flush_describes_only_the_frames_written() {
    let samples = signal(7, 16, 4096 * 2 * 20, 2, 6);
    for threads in [1, 4] {
        let mut enc = Encoder::new(cfg(2, 16, 5, threads)).unwrap();
        let (mut bytes, mut ends) = (Vec::new(), Vec::new());
        enc.write_ends(&samples, &mut bytes, &mut ends).unwrap();
        let dropped = enc.frames_in_flight() > 0;
        let info = enc.finish();
        assert_eq!(info.total_samples, 4096 * ends.len() as u64);
        let mut dec = Decoder::new(&info).unwrap();
        let mut start = 0;
        for &end in &ends {
            dec.decode_frame(&bytes[start..end]).unwrap();
            start = end;
        }
        let want = if dropped {
            Verification::NoMd5
        } else {
            Verification::Md5Matched
        };
        assert_eq!(dec.finish().unwrap(), want, "{threads} threads");
    }
}

#[test]
fn encode_block_waits_for_frames_in_flight_to_be_flushed() {
    let samples = signal(7, 16, 4096 * 2 * 12, 2, 7);
    let mut enc = Encoder::new(cfg(2, 16, 5, 4)).unwrap();
    let mut out = Vec::new();
    enc.write(&samples, &mut out).unwrap();
    if enc.frames_in_flight() > 0 {
        assert!(matches!(
            enc.encode_block(&samples[..8192], &mut out),
            Err(Error::Malformed(_))
        ));
    }
    enc.flush(&mut out).unwrap();
    enc.encode_block(&samples[..8192], &mut out).unwrap();
    let mut whole = samples.clone();
    whole.extend_from_slice(&samples[..8192]);
    assert_eq!(written(cfg(2, 16, 5, 1), &whole, 0).bytes, out);
}

#[test]
fn dropping_a_busy_encoder_returns() {
    let samples = signal(8, 24, 4096 * 2 * 30, 2, 8);
    for threads in [2, 8] {
        let mut enc = Encoder::new(cfg(2, 24, 8, threads)).unwrap();
        let mut out = Vec::new();
        enc.write(&samples, &mut out).unwrap();
        drop(enc);
    }
}
