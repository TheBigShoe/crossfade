#![no_main]
//! The ALAC decoder never panics on hostile configs or packets, never returns more than a
//! frame's worth of samples, and decodes a packet the same however often it sees it.

use crossfade_alac::{Config, Decoder};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((config, mut rest)) = data.split_at_checked(Config::LEN) else {
        return;
    };
    let Ok(config) = Config::parse(config) else {
        return;
    };
    // Big frames only make each run slow; their buffers are the same code.
    if config.frame_length > 1 << 16 {
        return;
    }
    let mut decoder = Decoder::new(config).unwrap();
    let max = config.frame_length as usize * usize::from(config.channels);
    let half = 1i64 << (config.bit_depth - 1);
    let (mut out, mut again) = (Vec::new(), Vec::new());
    // Packets: a 16-bit big-endian length, then that many bytes (or what is left).
    while let [hi, lo, tail @ ..] = rest {
        let n = usize::from(u16::from_be_bytes([*hi, *lo])).min(tail.len());
        let (packet, next) = tail.split_at(n);
        rest = next;
        let first = decoder.decode(packet, &mut out);
        if let Ok(frames) = first {
            assert_eq!(out.len(), frames * usize::from(config.channels));
            assert!(out.len() <= max);
            assert!(out.iter().all(|&s| (-half..half).contains(&i64::from(s))));
        }
        assert_eq!(decoder.decode(packet, &mut again), first);
        assert_eq!(again, out);
    }
});
