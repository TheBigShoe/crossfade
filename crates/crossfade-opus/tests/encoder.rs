//! The public encoder against the public decoder: packets that parse, decode to the encoder's
//! final range and sound like the input.

use crossfade_opus::{Bandwidth, Decoder, Encoder, Mode, OpusHead, Packet};

fn tones(len: usize, channels: usize) -> Vec<f32> {
    let tau = std::f64::consts::TAU;
    let mut pcm = Vec::with_capacity(len * channels);
    for i in 0..len {
        let t = i as f64 / 48000.0;
        let a = 0.3 * libm::sin(tau * 440.0 * t) + 0.1 * libm::sin(tau * 3100.0 * t + 0.4);
        let b = 0.2 * libm::sin(tau * 660.0 * t + 1.0) + 0.1 * libm::sin(tau * 3100.0 * t);
        pcm.push(a as f32);
        if channels == 2 {
            pcm.push(b as f32);
        }
    }
    pcm
}

fn snr_db(want: &[f32], got: &[f32], channels: usize, delay: usize, skip: usize) -> f64 {
    let (mut sig, mut err) = (0.0f64, 0.0f64);
    for i in skip + delay..got.len() / channels {
        for ch in 0..channels {
            let w = f64::from(want[(i - delay) * channels + ch]);
            let g = f64::from(got[i * channels + ch]);
            sig += w * w;
            err += (w - g) * (w - g);
        }
    }
    10.0 * libm::log10(sig / err.max(1e-30))
}

#[test]
fn packets_are_celt_fullband_of_the_bitrate_and_decode_to_the_final_range() {
    for channels in 1..=2 {
        for (frame_size, bitrate) in [(960, 96_000), (480, 64_000), (240, 128_000), (120, 256_000)]
        {
            let mut enc = Encoder::new(channels).unwrap();
            enc.set_bitrate(bitrate).unwrap();
            enc.set_frame_size(frame_size).unwrap();
            assert_eq!((enc.bitrate(), enc.frame_size()), (bitrate, frame_size));
            let mut dec = Decoder::new(channels).unwrap();
            let pcm = tones(frame_size * 30, channels);
            let mut out = Vec::new();
            for chunk in pcm.chunks(frame_size * channels) {
                let mut packet = Vec::new();
                enc.encode(chunk, &mut packet).unwrap();
                // Constant bitrate: every packet the same size, TOC included.
                assert_eq!(
                    packet.len(),
                    (bitrate as usize * frame_size / 48_000).div_ceil(8)
                );
                let parsed = Packet::parse(&packet).unwrap();
                assert_eq!(parsed.toc().mode(), Mode::Celt);
                assert_eq!(parsed.toc().bandwidth(), Bandwidth::Full);
                assert_eq!(parsed.toc().stereo(), channels == 2);
                assert_eq!((parsed.frames().len(), parsed.duration()), (1, frame_size));
                assert_eq!(dec.decode(Some(&packet), &mut out).unwrap(), frame_size);
                assert_eq!(dec.final_range(), enc.final_range());
            }
            let snr = snr_db(&pcm, &out, channels, enc.lookahead(), frame_size * 4);
            assert!(
                snr > 20.0,
                "{channels} ch {frame_size} at {bitrate}: {snr:.1} dB"
            );
        }
    }
}

#[test]
fn settings_out_of_range_are_refused() {
    assert!(Encoder::new(0).is_err());
    assert!(Encoder::new(3).is_err());
    let mut enc = Encoder::new(2).unwrap();
    assert!(enc.set_bitrate(5_999).is_err());
    assert!(enc.set_bitrate(510_001).is_err());
    assert!(enc.set_frame_size(1920).is_err());
    assert!(enc.set_frame_size(100).is_err());
    let mut packet = vec![1, 2, 3];
    assert!(enc.encode(&[0.0; 100], &mut packet).is_err());
    assert_eq!(packet, [1, 2, 3], "a refused frame appends nothing");
    // The defaults: 20 ms, 64 kb/s a channel.
    assert_eq!((enc.frame_size(), enc.bitrate()), (960, 128_000));
    assert_eq!(Encoder::new(1).unwrap().bitrate(), 64_000);
}

#[test]
fn a_packet_is_appended_and_reset_starts_the_stream_again() {
    let pcm = tones(960 * 5, 1);
    let mut enc = Encoder::new(1).unwrap();
    let run = |enc: &mut Encoder| {
        let mut all = vec![0xAA];
        for chunk in pcm.chunks(960) {
            enc.encode(chunk, &mut all).unwrap();
        }
        all
    };
    let first = run(&mut enc);
    assert_eq!(first[0], 0xAA);
    assert_eq!(first.len(), 1 + 5 * 160);
    enc.reset();
    assert_eq!(run(&mut enc), first);
}

#[test]
fn the_identification_header_says_what_the_encoder_does() {
    let enc = Encoder::new(2).unwrap();
    let head = enc.head(44_100);
    assert_eq!(usize::from(head.pre_skip), enc.lookahead());
    assert_eq!(
        (head.channels, head.input_sample_rate, head.output_gain),
        (2, 44_100, 0)
    );
    assert_eq!(
        (
            head.mapping.family,
            head.mapping.streams,
            head.mapping.coupled
        ),
        (0, 1, 1)
    );
    let bytes = head.to_bytes();
    assert_eq!(bytes.len(), 19);
    assert_eq!(OpusHead::parse(&bytes).unwrap(), head);
    let mono = Encoder::new(1).unwrap().head(48_000);
    assert_eq!(OpusHead::parse(&mono.to_bytes()).unwrap(), mono);
}
