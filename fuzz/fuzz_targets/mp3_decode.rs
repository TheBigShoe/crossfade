#![no_main]
//! Frames straight into the MP3 decoder: never a panic, and whatever the bytes, every sample
//! out is finite and within the documented bound, and every frame that parses as a header
//! gives exactly its samples.

use crossfade_mp3::{Decoder, FrameHeader, OUTPUT_LIMIT};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut decoder = Decoder::new();
    let mut out = Vec::new();
    let mut rest = data;
    // Each frame: two bytes of length (any, to reach the length checks too), then the frame.
    while let [hi, lo, tail @ ..] = rest {
        let len = (usize::from(*hi) << 8 | usize::from(*lo)) % 4200;
        let (frame, after) = tail.split_at(len.min(tail.len()));
        rest = after;
        if frame.first() == Some(&0) {
            decoder.reset();
        }
        out.clear();
        let header = frame.first_chunk::<4>().and_then(|h| FrameHeader::parse(h).ok());
        let _ = decoder.decode_frame(frame, &mut out);
        let want = header.map_or(0, |h| h.samples_per_frame() as usize * usize::from(h.channels()));
        assert_eq!(out.len(), want);
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= OUTPUT_LIMIT));
    }
});
