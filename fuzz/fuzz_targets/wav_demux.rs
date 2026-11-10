#![no_main]

use crossfade_core::{Demuxer, Limits, Packet};
use crossfade_format::wav::WavDemuxer;
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_alloc_bytes: 1 << 20,
        max_nesting_depth: 8,
        max_packet_bytes: 1 << 16,
    };
    let Ok(mut demuxer) = WavDemuxer::new(Box::new(Cursor::new(data.to_vec())), &limits) else {
        return;
    };
    let bytes_per_frame = demuxer.streams()[0].audio.bytes_per_frame();
    let mut packet = Packet::default();
    let mut total = 0usize;
    // Every successful call consumes input, so this bounds the loop.
    for _ in 0..data.len() + 2 {
        match demuxer.read_packet(&mut packet) {
            Ok(true) => {
                assert_eq!(packet.data.len() % bytes_per_frame, 0);
                total += packet.data.len();
                assert!(total <= data.len());
            }
            _ => return,
        }
    }
    panic!("read loop did not end");
});
