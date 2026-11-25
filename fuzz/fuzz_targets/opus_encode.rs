#![no_main]
//! The Opus encoders on any config and any PCM bits (NaN and infinities included), with
//! bitrate changes and resets: never a panic; every packet parses, lasts one frame, and
//! decodes with the encoder's final range (every stream's, for a multistream packet). The
//! input: channels (1 to 8; from 3, or by a flag, the multistream encoder), bitrate, VBR,
//! frame duration (2.5 to 60 ms), signal, application, a forced mode and bandwidth, FEC,
//! loss, DTX and the input depth (any byte: outside 8 to 24 bits it must be refused) from the
//! first bytes, then frames of raw `f32` bits, a reset or a new bitrate between some.

use arbitrary::{Arbitrary, Unstructured};
use crossfade_opus::{
    Application, Bandwidth, Decoder, Encoder, EncoderConfig, FrameDuration, Mode,
    MultistreamDecoder, MultistreamEncoder, Packet, Signal,
};
use libfuzzer_sys::fuzz_target;

enum Enc {
    One(Encoder, Decoder),
    Multi(MultistreamEncoder, MultistreamDecoder),
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let (Ok(channels), Ok(bitrate), Ok(flags), Ok(more), Ok(depth)) = (
        u.int_in_range(1u8..=8),
        u.int_in_range(1u32..=700_000),
        u8::arbitrary(&mut u),
        u16::arbitrary(&mut u),
        u8::arbitrary(&mut u),
    ) else {
        return;
    };
    let mut config = EncoderConfig::default();
    config.channels = channels;
    config.bitrate = bitrate;
    config.vbr = flags & 1 != 0;
    config.constrained_vbr = flags & 2 != 0;
    config.frame_duration = match (flags >> 2) & 7 {
        0 => FrameDuration::Ms2_5,
        1 => FrameDuration::Ms5,
        2 => FrameDuration::Ms10,
        3 | 6 => FrameDuration::Ms20,
        4 => FrameDuration::Ms40,
        _ => FrameDuration::Ms60,
    };
    config.signal = [Signal::Auto, Signal::Voice, Signal::Music, Signal::Auto][usize::from(more & 3)];
    if more & 4 != 0 {
        config.application = Application::Voip;
    }
    config.mode = [None, Some(Mode::Silk), Some(Mode::Hybrid), Some(Mode::Celt)]
        [usize::from((more >> 3) & 3)];
    config.bandwidth = [
        None,
        Some(Bandwidth::Narrow),
        Some(Bandwidth::Medium),
        Some(Bandwidth::Wide),
        Some(Bandwidth::SuperWide),
        Some(Bandwidth::Full),
        None,
        None,
    ][usize::from((more >> 5) & 7)];
    config.inband_fec = more & 0x100 != 0;
    config.packet_loss_percent = ((more >> 9) & 0x1f) as u8 * 3;
    config.dtx = more & 0x4000 != 0;
    // Mostly 16 and 24, the depths sources have; any other byte now and then.
    config.lsb_depth = match depth >> 6 {
        0 => 16,
        1 => 24,
        _ => depth & 0x1f,
    };
    let ch = usize::from(channels);
    let multi = channels > 2 || flags & 0x80 != 0;
    if !(8..=24).contains(&config.lsb_depth) {
        assert!(MultistreamEncoder::new(&config, u8::from(channels > 2)).is_err());
        assert!(channels > 2 || Encoder::new(&config).is_err());
        return;
    }
    let mut coder = if multi {
        let enc = MultistreamEncoder::new(&config, if channels > 2 { 1 } else { 0 }).unwrap();
        let head = enc.head(48_000);
        assert_eq!(crossfade_opus::OpusHead::parse(&head.to_bytes()).unwrap(), head);
        let dec = MultistreamDecoder::new(&head.mapping, ch).unwrap();
        Enc::Multi(enc, dec)
    } else {
        Enc::One(Encoder::new(&config).unwrap(), Decoder::new(ch).unwrap())
    };
    let n = config.frame_duration.samples();
    let mut pcm = vec![0.0f32; n * ch];
    let mut packet = vec![];
    let mut out = vec![];
    for _ in 0..12 {
        let Ok(ctl) = u8::arbitrary(&mut u) else {
            break;
        };
        if ctl == 0 {
            match &mut coder {
                Enc::One(e, d) => {
                    e.reset();
                    d.reset();
                }
                Enc::Multi(e, d) => {
                    e.reset();
                    d.reset();
                }
            }
            continue;
        }
        if ctl & 0xC0 == 0xC0 {
            let b = u.int_in_range(1u32..=700_000).unwrap_or(64_000);
            match &mut coder {
                Enc::One(e, _) => e.set_bitrate(b).unwrap(),
                Enc::Multi(e, _) => e.set_bitrate(b).unwrap(),
            }
        }
        // Raw bits where the input has them, a scaled ramp after.
        for (i, s) in pcm.iter_mut().enumerate() {
            *s = match u32::arbitrary(&mut u) {
                Ok(bits) if ctl & 1 != 0 => f32::from_bits(bits),
                Ok(bits) => (bits as i32) as f32 / i32::MAX as f32,
                Err(_) => (i as f32 * 0.01).sin() * f32::from(ctl) / 64.0,
            };
        }
        out.clear();
        match &mut coder {
            Enc::One(e, d) => {
                e.encode(&pcm, &mut packet).unwrap();
                assert_eq!(Packet::parse(&packet).unwrap().duration(), n);
                assert_eq!(d.decode(Some(&packet), &mut out).unwrap(), n);
                assert_eq!(d.final_range(), e.final_range());
            }
            Enc::Multi(e, d) => {
                e.encode(&pcm, &mut packet).unwrap();
                assert_eq!(d.decode(Some(&packet), &mut out).unwrap(), n);
                assert_eq!(d.final_ranges(), e.final_ranges());
            }
        }
        assert!(out.iter().all(|s| s.is_finite()));
    }
});
