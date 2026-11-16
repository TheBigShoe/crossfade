#![no_main]
//! Any samples through the ALAC encoder: the input chooses the channel count, the depth and
//! each packet's length, then gives the samples. Never a panic; every packet decodes back to
//! exactly the samples that went in.

use arbitrary::{Result, Unstructured};
use crossfade_alac::{Config, Decoder, Encoder};
use libfuzzer_sys::fuzz_target;

fn run(data: &[u8]) -> Result<()> {
    let mut u = Unstructured::new(data);
    let channels = u.int_in_range(1..=8u8)?;
    let depth = *u.choose(&[16u8, 20, 24, 32])?;
    let config = Config::new(44_100, channels, depth).expect("a stream ALAC writes");
    let mut enc = Encoder::new(config).expect("a checked config");
    let mut dec = Decoder::new(config).expect("a checked config");
    let mut packets = 0;
    while !u.is_empty() && packets < 4 {
        let frames = u.int_in_range(1..=config.frame_length as usize)?;
        let mut pcm = vec![0i32; frames * usize::from(channels)];
        for v in &mut pcm {
            // Out of input: the rest of the packet stays silent.
            *v = u.arbitrary::<i32>().unwrap_or(0) >> (32 - u32::from(depth));
        }
        let mut packet = vec![];
        enc.encode(&pcm, &mut packet).expect("any samples encode");
        let mut out = vec![];
        assert_eq!(dec.decode(&packet, &mut out), Ok(frames));
        assert_eq!(out, pcm);
        packets += 1;
    }
    Ok(())
}

fuzz_target!(|data: &[u8]| {
    let _ = run(data);
});
