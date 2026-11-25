use super::*;
use crate::opus::decoder;
use crossfade_core::{
    AudioFrame, ChannelLayout, CodecId, Packet, SampleFormat, Samples, StreamInfo, TimeBase,
};
use crossfade_opus::{OpusHead, packet_duration};

fn spec(channels: u16) -> AudioSpec {
    let layout = match channels {
        1 => ChannelLayout::mono(),
        2 => ChannelLayout::stereo(),
        n => ChannelLayout::from_name(if n == 6 { "5.1" } else { "quad" }).unwrap(),
    };
    AudioSpec::new(48_000, SampleFormat::F32, layout).unwrap()
}

fn what(r: Result<Box<dyn Encoder>, Error>) -> String {
    match r {
        Err(Error::Unsupported { what }) => what,
        Err(e) => panic!("wrong error {e:?}"),
        Ok(_) => panic!("expected Unsupported"),
    }
}

/// A chirp from 200 Hz up past 8 kHz in `frames` frames, the second channel a little lower:
/// broadband enough that a shift of a few samples shows in the correlation.
fn chirp(frames: usize, channels: usize) -> Vec<f32> {
    let mut v = Vec::with_capacity(frames * channels);
    let secs = (frames as f64 / 48_000.0).max(0.001);
    for i in 0..frames {
        let t = i as f64 / 48_000.0;
        for c in 0..channels {
            let f0 = 200.0;
            let f1 = 8_000.0 - 1_500.0 * c as f64;
            let phase = 2.0 * std::f64::consts::PI * (f0 * t + (f1 - f0) * t * t / (2.0 * secs));
            v.push((0.4 * phase.sin()) as f32);
        }
    }
    v
}

fn frame(spec: &AudioSpec, v: &[f32]) -> AudioFrame {
    AudioFrame {
        samples: Samples::F32(v.to_vec()),
        ..AudioFrame::new(spec)
    }
}

fn encode_with(
    spec: &AudioSpec,
    options: &EncodeOptions,
    pcm: &[f32],
    piece: usize,
) -> (Vec<Packet>, Vec<u8>) {
    let mut e = encoder(spec, options).unwrap();
    let n = usize::from(spec.layout().channels());
    let mut packets = Vec::new();
    let mut p = Packet::default();
    for chunk in pcm.chunks(piece * n) {
        e.send_frame(&frame(spec, chunk)).unwrap();
        while e.receive_packet(&mut p).unwrap() {
            packets.push(p.clone());
        }
    }
    e.drain();
    while e.receive_packet(&mut p).unwrap() {
        packets.push(p.clone());
    }
    (packets, e.extradata().to_vec())
}

#[test]
fn only_48_khz_float_mono_and_stereo_are_encoded_and_the_refusals_say_why() {
    for f in [SampleFormat::S16, SampleFormat::S24, SampleFormat::F64] {
        let s = AudioSpec::new(48_000, f, ChannelLayout::stereo()).unwrap();
        let w = what(encoder(&s, &EncodeOptions::default()));
        assert!(w.contains("opus") && w.contains("f32"), "{w}");
    }
    let s = AudioSpec::new(44_100, SampleFormat::F32, ChannelLayout::stereo()).unwrap();
    let w = what(encoder(&s, &EncodeOptions::default()));
    assert!(w.contains("48000 Hz") && w.contains("44100"), "{w}");
    for n in [4, 6] {
        let w = what(encoder(&spec(n), &EncodeOptions::default()));
        assert!(
            w.contains("surround")
                && w.contains("not written yet")
                && w.contains("--channels stereo"),
            "{w}"
        );
    }
    assert!(encoder(&spec(1), &EncodeOptions::default()).is_ok());
    assert!(encoder(&spec(2), &EncodeOptions::default()).is_ok());
}

#[test]
fn the_header_has_the_pre_skip_the_channels_and_the_original_rate() {
    assert_eq!(pre_skip(), 312);
    for (channels, rate, want) in [(1, Some(44_100), 44_100), (2, None, 48_000)] {
        let options = EncodeOptions {
            input_sample_rate: rate,
            ..EncodeOptions::default()
        };
        let e = encoder(&spec(channels), &options).unwrap();
        let h = OpusHead::parse(e.extradata()).unwrap();
        assert_eq!(u16::from(h.channels), channels);
        assert_eq!(h.pre_skip, 312);
        assert_eq!(h.input_sample_rate, want);
        assert_eq!(h.output_gain, 0);
        assert_eq!(h.mapping.family, 0);
    }
    let options = EncodeOptions {
        input_sample_rate: Some(44_100),
        ..EncodeOptions::default()
    };
    let e = encoder(&spec(2), &options).unwrap();
    assert_eq!(head(2, 44_100), e.extradata());
}

#[test]
fn packets_start_at_minus_the_pre_skip_and_the_last_ends_at_the_audible_end() {
    for (frames, count) in [(2500usize, 3usize), (648, 1), (649, 2), (100, 1), (5000, 6)] {
        let (packets, _) = encode_with(&spec(2), &EncodeOptions::default(), &chirp(frames, 2), 700);
        assert_eq!(packets.len(), count, "{frames}");
        assert_eq!(packets[0].pts, -312, "{frames}");
        for w in packets.windows(2) {
            let toc = packet_duration(&w[0].data).unwrap() as u64;
            assert_eq!(w[0].duration, toc);
            assert_eq!(w[1].pts, w[0].pts + toc as i64);
        }
        let last = packets.last().unwrap();
        assert!(last.duration <= packet_duration(&last.data).unwrap() as u64);
        assert_eq!(last.pts + last.duration as i64, frames as i64, "{frames}");
    }
}

#[test]
fn no_input_makes_no_packet() {
    let (packets, extradata) = encode_with(&spec(1), &EncodeOptions::default(), &[], 960);
    assert!(packets.is_empty());
    assert_eq!(extradata.len(), 19);
}

/// Encodes `pcm`, decodes it through the decoder adapter and trims what the timeline would.
fn round_trip(channels: u16, options: &EncodeOptions, pcm: &[f32]) -> Vec<f32> {
    let spec = spec(channels);
    let (packets, extradata) = encode_with(&spec, options, pcm, 1000);
    let mut stream = StreamInfo::new(
        0,
        CodecId::Opus,
        spec.clone(),
        TimeBase::new(1, 48_000).unwrap(),
    );
    stream.extradata = extradata;
    let mut d = decoder(&stream).unwrap();
    let mut out = Vec::new();
    let mut f = AudioFrame::new(&spec);
    let ch = usize::from(channels);
    for p in &packets {
        d.send_packet(p).unwrap();
        while d.receive_frame(&mut f).unwrap() {
            let Samples::F32(s) = &f.samples else {
                panic!()
            };
            for (i, x) in s.chunks_exact(ch).enumerate() {
                if f.pts + i as i64 >= 0 {
                    out.extend_from_slice(x);
                }
            }
        }
    }
    let end = packets.last().map_or(0, |p| p.pts + p.duration as i64) as usize;
    out.truncate(end * ch);
    out
}

fn snr_db(a: &[f32], b: &[f32]) -> f64 {
    let sig: f64 = a.iter().map(|&x| f64::from(x).powi(2)).sum();
    let err: f64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| f64::from(x - y).powi(2))
        .sum();
    10.0 * (sig / err.max(1e-30)).log10()
}

#[test]
fn what_the_decoder_gives_back_is_as_long_as_the_input_and_not_shifted() {
    for channels in [1u16, 2] {
        let ch = usize::from(channels);
        let frames = 24_000 + 313;
        let pcm = chirp(frames, ch);
        let got = round_trip(channels, &EncodeOptions::default(), &pcm);
        assert_eq!(got.len(), pcm.len(), "{channels} ch");
        // Correlate at lags of -400..=400 frames: the best is no shift.
        let lag_score = |lag: i64| -> f64 {
            (0..frames as i64)
                .filter(|&i| (0..frames as i64).contains(&(i + lag)))
                .map(|i| f64::from(pcm[i as usize * ch]) * f64::from(got[(i + lag) as usize * ch]))
                .sum()
        };
        let best = (-400..=400)
            .max_by(|&a, &b| lag_score(a).total_cmp(&lag_score(b)))
            .unwrap();
        assert_eq!(best, 0, "{channels} ch");
        let snr = snr_db(&pcm, &got);
        // Measured 37.9 dB (mono) and 28.3 dB (stereo) at 128 kb/s.
        assert!(snr > 20.0, "{channels} ch: {snr:.1} dB");
    }
}

#[test]
fn bitrate_sets_the_size_and_constant_bitrate_makes_every_packet_alike() {
    let pcm = chirp(48_000, 2);
    let size = |bitrate| {
        let options = EncodeOptions {
            bitrate: Some(bitrate),
            ..EncodeOptions::default()
        };
        let (packets, _) = encode_with(&spec(2), &options, &pcm, 960);
        packets.iter().map(|p| p.data.len()).sum::<usize>()
    };
    let (low, high) = (size(32_000), size(256_000));
    assert!(low * 3 < high, "{low} {high}");
    let options = EncodeOptions {
        bitrate: Some(128_000),
        constant_bitrate: true,
        ..EncodeOptions::default()
    };
    let (packets, _) = encode_with(&spec(2), &options, &pcm, 960);
    assert!(packets.len() > 40);
    let sizes: Vec<usize> = packets.iter().map(|p| p.data.len()).collect();
    assert!(sizes.iter().all(|&n| n == 320), "{sizes:?}");
    assert_eq!(default_bitrate(1), 96_000);
    assert_eq!(default_bitrate(2), 128_000);
}

#[test]
fn the_bytes_do_not_depend_on_how_the_frames_arrive() {
    let pcm = chirp(10_000, 2);
    let a = encode_with(&spec(2), &EncodeOptions::default(), &pcm, 960).0;
    for piece in [1, 333, 4096, 20_000] {
        let b = encode_with(&spec(2), &EncodeOptions::default(), &pcm, piece).0;
        assert_eq!(a, b, "{piece}");
    }
}

#[test]
fn a_frame_that_is_not_what_the_encoder_was_built_for_is_refused() {
    let mut e = encoder(&spec(2), &EncodeOptions::default()).unwrap();
    let wrong = frame(&spec(1), &[0.0; 10]);
    assert!(matches!(
        e.send_frame(&wrong),
        Err(Error::Unsupported { .. })
    ));
    let s16 = AudioSpec::new(48_000, SampleFormat::S16, ChannelLayout::stereo()).unwrap();
    let f = AudioFrame {
        samples: Samples::S16(vec![0; 10]),
        ..AudioFrame::new(&s16)
    };
    assert!(matches!(e.send_frame(&f), Err(Error::Unsupported { .. })));
    e.drain();
    let late = frame(&spec(2), &[0.0; 4]);
    assert!(matches!(
        e.send_frame(&late),
        Err(Error::Unsupported { .. })
    ));
}
