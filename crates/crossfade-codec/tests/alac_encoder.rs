//! The engine's ALAC encoder: integer frames of any size in, 4096-frame packets out, and the
//! engine's ALAC decoder gives every sample back.

use crossfade_codec::{EncodeOptions, decoder_for, encoder_for};
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Error, Packet, SampleFormat, Samples,
    StreamInfo, TimeBase,
};

fn spec(rate: u32, format: SampleFormat, layout: ChannelLayout) -> AudioSpec {
    AudioSpec::new(rate, format, layout).unwrap()
}

/// ALAC's own layout for `channels`, as the reader describes a stream.
fn alac_layout(channels: u8) -> ChannelLayout {
    let wave = crossfade_alac::wave_layout(channels).unwrap();
    ChannelLayout::from_mask(wave.mask, u16::from(channels)).unwrap()
}

/// Samples of `bits` true bits, left-justified in `format`, a different tone per channel.
fn samples(format: SampleFormat, bits: u32, frames: usize, channels: usize) -> Samples {
    let width = format.bits() as u32;
    let value = |i: usize, c: usize| -> i32 {
        let t = i as f64 / 44_100.0;
        let v = 0.4 * (std::f64::consts::TAU * (300.0 + 90.0 * c as f64) * t).sin();
        let raw = (v * f64::from(1u32 << (bits - 1))) as i32;
        raw << (width - bits)
    };
    let all = (0..frames).flat_map(|i| (0..channels).map(move |c| value(i, c)));
    match format {
        SampleFormat::S16 => Samples::S16(all.map(|v| v as i16).collect()),
        SampleFormat::S24 => Samples::S24(all.collect()),
        _ => Samples::S32(all.collect()),
    }
}

fn slice(s: &Samples, range: std::ops::Range<usize>) -> Samples {
    match s {
        Samples::S16(v) => Samples::S16(v[range].to_vec()),
        Samples::S24(v) => Samples::S24(v[range].to_vec()),
        Samples::S32(v) => Samples::S32(v[range].to_vec()),
        _ => unreachable!(),
    }
}

fn append(to: &mut Samples, more: &Samples) {
    match (to, more) {
        (Samples::S16(a), Samples::S16(b)) => a.extend_from_slice(b),
        (Samples::S24(a), Samples::S24(b)) | (Samples::S32(a), Samples::S32(b)) => {
            a.extend_from_slice(b);
        }
        _ => panic!("the decoder changed format"),
    }
}

#[test]
fn every_depth_and_layout_round_trips_through_the_engines_decoder() {
    for (format, bits) in [
        (SampleFormat::S16, 16u8),
        (SampleFormat::S24, 24),
        (SampleFormat::S24, 20),
        (SampleFormat::S32, 32),
    ] {
        for channels in [1u8, 2, 6, 8] {
            let s = spec(44_100, format, alac_layout(channels));
            let ch = usize::from(channels);
            let frames = 4096 * 2 + 777;
            let pcm = samples(format, u32::from(bits), frames, ch);
            let options = EncodeOptions {
                bits_per_sample: Some(bits),
                ..EncodeOptions::default()
            };
            let mut enc = encoder_for(CodecId::Alac, &s, &options).unwrap();
            // The cookie is known before any audio: a muxer writes it first.
            let config = crossfade_alac::Config::parse(enc.extradata()).unwrap();
            assert_eq!(
                (config.bit_depth, config.channels, config.sample_rate),
                (bits, channels, 44_100)
            );

            let mut packets = vec![];
            let mut p = Packet::default();
            let mut frame = AudioFrame::new(&s);
            let mut at = 0;
            for size in [1000usize, 5000, 3000, 8192].into_iter().cycle() {
                if at == frames {
                    break;
                }
                let n = size.min(frames - at);
                frame.pts = 100 + at as i64;
                frame.samples = slice(&pcm, at * ch..(at + n) * ch);
                at += n;
                enc.send_frame(&frame).unwrap();
                while enc.receive_packet(&mut p).unwrap() {
                    packets.push(p.clone());
                }
            }
            enc.drain();
            while enc.receive_packet(&mut p).unwrap() {
                packets.push(p.clone());
            }
            let what = format!("{format:?} {bits}-bit {channels} ch");
            let shape: Vec<(i64, u64)> = packets.iter().map(|p| (p.pts, p.duration)).collect();
            assert_eq!(
                shape,
                [(100, 4096), (100 + 4096, 4096), (100 + 8192, 777)],
                "{what}"
            );

            let mut stream = StreamInfo::new(
                0,
                CodecId::Alac,
                s.clone(),
                TimeBase::new(1, 44_100).unwrap(),
            );
            stream.extradata = enc.extradata().to_vec();
            stream.bits_per_raw_sample = (bits == 20).then_some(20);
            let mut dec = decoder_for(&stream).unwrap();
            let mut out = AudioFrame::new(&s);
            let mut got = Samples::new(format);
            for p in &packets {
                dec.send_packet(p).unwrap();
                while dec.receive_frame(&mut out).unwrap() {
                    append(&mut got, &out.samples);
                }
            }
            dec.drain();
            while dec.receive_frame(&mut out).unwrap() {
                append(&mut got, &out.samples);
            }
            assert!(got == pcm, "{what}: other samples came back");
        }
    }
}

#[test]
fn what_alac_cannot_hold_is_refused_by_name() {
    let refused = |s: AudioSpec, bits: Option<u8>| {
        let options = EncodeOptions {
            bits_per_sample: bits,
            ..EncodeOptions::default()
        };
        match encoder_for(CodecId::Alac, &s, &options) {
            Err(Error::Unsupported { what }) => what,
            Err(e) => panic!("{e:?}"),
            Ok(_) => panic!("accepted"),
        }
    };
    assert_eq!(
        refused(
            spec(44_100, SampleFormat::F32, ChannelLayout::stereo()),
            None
        ),
        "alac cannot store floating-point samples; choose --sample-format s16, s24 or s32"
    );
    assert_eq!(
        refused(
            spec(44_100, SampleFormat::U8, ChannelLayout::stereo()),
            None
        ),
        "alac holds 16-, 20-, 24- and 32-bit audio, not 8-bit; choose --sample-format s16"
    );
    assert_eq!(
        refused(
            spec(44_100, SampleFormat::S24, ChannelLayout::stereo()),
            Some(18)
        ),
        "alac holds 16-, 20-, 24- and 32-bit audio, not 18-bit"
    );
    let back = ChannelLayout::from_name("5.1-back").unwrap();
    assert_eq!(
        refused(spec(48_000, SampleFormat::S16, back), None),
        "alac cannot hold the layout 5.1-back (for 6 channels it has 5.1)"
    );
    assert_eq!(
        refused(
            spec(
                48_000,
                SampleFormat::S16,
                ChannelLayout::unpositioned(9).unwrap()
            ),
            None
        ),
        "alac holds 1 to 8 channels, not 9"
    );
}
