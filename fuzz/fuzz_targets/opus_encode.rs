#![no_main]
//! Any samples at any setting through the Opus encoder: the input chooses channels, frame
//! size and bitrate, then gives the raw bits of every sample (NaNs and infinities included).
//! Never a panic or a hang; every frame becomes a packet of its duration that the decoder
//! reads to the encoder's final range, as finite samples.

use arbitrary::{Result, Unstructured};
use crossfade_opus::{Decoder, Encoder, packet_duration};
use libfuzzer_sys::fuzz_target;

fn run(data: &[u8]) -> Result<()> {
    let mut u = Unstructured::new(data);
    let channels = if u.arbitrary::<bool>()? { 2 } else { 1 };
    let frame_size = 120usize << u.int_in_range(0..=3u32)?;
    let bitrate = u.int_in_range(6_000..=510_000u32)?;
    let mut enc = Encoder::new(channels).expect("1 or 2 channels");
    enc.set_bitrate(bitrate).expect("in range");
    enc.set_frame_size(frame_size).expect("a CELT frame size");
    let mut dec = Decoder::new(channels).expect("1 or 2 channels");
    let mut frames = 0;
    while !u.is_empty() && frames < 8 {
        let mut pcm = vec![0.0f32; frame_size * channels];
        for v in &mut pcm {
            // Out of input: the rest of the frame stays silent.
            *v = f32::from_bits(u.arbitrary::<u32>().unwrap_or(0));
        }
        let mut packet = vec![];
        enc.encode(&pcm, &mut packet).expect("any samples encode");
        assert_eq!(packet_duration(&packet), Ok(frame_size));
        let mut out = vec![];
        assert_eq!(dec.decode(Some(&packet), &mut out), Ok(frame_size));
        assert_eq!(dec.final_range(), enc.final_range());
        assert!(out.iter().all(|v| v.is_finite()));
        frames += 1;
    }
    Ok(())
}

fuzz_target!(|data: &[u8]| {
    let _ = run(data);
});
