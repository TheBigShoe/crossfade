#![no_main]
//! The Opus encoder on any config and any PCM bits (NaN and infinities included), with
//! resets: never a panic; every packet parses, lasts one frame, and decodes with the
//! encoder's final range. The input: channels, bitrate, VBR flags and frame duration from
//! the first bytes, then frames of raw `f32` bits, a reset between some.

use arbitrary::{Arbitrary, Unstructured};
use crossfade_opus::{Decoder, Encoder, EncoderConfig, FrameDuration, Packet};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let (Ok(channels), Ok(bitrate), Ok(flags)) = (
        u.int_in_range(1u8..=2),
        u.int_in_range(1u32..=700_000),
        u8::arbitrary(&mut u),
    ) else {
        return;
    };
    let mut config = EncoderConfig::default();
    config.channels = channels;
    config.bitrate = bitrate;
    config.vbr = flags & 1 != 0;
    config.constrained_vbr = flags & 2 != 0;
    config.frame_duration = match (flags >> 2) & 3 {
        0 => FrameDuration::Ms2_5,
        1 => FrameDuration::Ms5,
        2 => FrameDuration::Ms10,
        _ => FrameDuration::Ms20,
    };
    let mut enc = Encoder::new(&config).unwrap();
    let mut dec = Decoder::new(usize::from(channels)).unwrap();
    let samples = enc.frame_size() * usize::from(channels);
    let mut pcm = vec![0.0f32; samples];
    let mut packet = vec![];
    let mut out = vec![];
    for _ in 0..16 {
        let Ok(ctl) = u8::arbitrary(&mut u) else {
            break;
        };
        if ctl == 0 {
            enc.reset();
            dec.reset();
            continue;
        }
        // Raw bits where the input has them, a scaled ramp after.
        for (i, s) in pcm.iter_mut().enumerate() {
            *s = match u32::arbitrary(&mut u) {
                Ok(bits) if ctl & 1 != 0 => f32::from_bits(bits),
                Ok(bits) => (bits as i32) as f32 / i32::MAX as f32,
                Err(_) => (i as f32 * 0.01).sin() * f32::from(ctl) / 64.0,
            };
        }
        enc.encode(&pcm, &mut packet).unwrap();
        let parsed = Packet::parse(&packet).unwrap();
        assert_eq!(parsed.duration(), enc.frame_size());
        out.clear();
        assert_eq!(dec.decode(Some(&packet), &mut out).unwrap(), enc.frame_size());
        assert_eq!(dec.final_range(), enc.final_range());
        assert!(out.iter().all(|s| s.is_finite()));
    }
});
