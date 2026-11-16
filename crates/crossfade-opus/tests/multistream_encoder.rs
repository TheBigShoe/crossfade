//! The multistream encoder against the multistream decoder: 1 to 8 channels in Vorbis order
//! (mapping family 1), every stream's packet in one, decoded back to the input.

use crossfade_opus::{MultistreamDecoder, MultistreamEncoder, OpusHead, packet_duration};

/// A different tone in each channel, so that a swapped channel shows.
fn tones(frames: usize, channels: usize) -> Vec<f32> {
    let mut pcm = Vec::with_capacity(frames * channels);
    for i in 0..frames {
        let t = i as f64 / 48_000.0;
        for ch in 0..channels {
            let hz = 300.0 + 170.0 * ch as f64;
            pcm.push((0.3 * libm::sin(std::f64::consts::TAU * hz * t)) as f32);
        }
    }
    pcm
}

fn snr_db(want: &[f32], got: &[f32], channels: usize, ch: usize, delay: usize) -> f64 {
    let (mut sig, mut err) = (0.0f64, 0.0f64);
    for i in 4800 + delay..got.len() / channels {
        let w = f64::from(want[(i - delay) * channels + ch]);
        let g = f64::from(got[i * channels + ch]);
        sig += w * w;
        err += (w - g) * (w - g);
    }
    10.0 * libm::log10(sig / err.max(1e-30))
}

#[test]
fn every_channel_count_decodes_back_channel_for_channel() {
    // Streams and coupled streams per channel count, as RFC 7845 5.1.1.2 lays them out.
    let layout = [
        (1, 0),
        (1, 1),
        (2, 1),
        (2, 2),
        (3, 2),
        (4, 2),
        (5, 2),
        (5, 3),
    ];
    for channels in 1..=8usize {
        let mut enc = MultistreamEncoder::new(channels).unwrap();
        let head = enc.head(48_000);
        assert_eq!(usize::from(head.channels), channels);
        assert_eq!(head.mapping.family, u8::from(channels > 2));
        assert_eq!(
            (
                usize::from(head.mapping.streams),
                usize::from(head.mapping.coupled)
            ),
            layout[channels - 1],
            "{channels} channels"
        );
        assert_eq!(OpusHead::parse(&head.to_bytes()).unwrap(), head);

        let mut dec = MultistreamDecoder::new(&head.mapping, channels).unwrap();
        let pcm = tones(960 * 25, channels);
        let mut out = vec![];
        for chunk in pcm.chunks(960 * channels) {
            let mut packet = vec![0x55];
            enc.encode(chunk, &mut packet).unwrap();
            assert_eq!(packet[0], 0x55, "the packet is appended");
            let packet = &packet[1..];
            assert_eq!(packet_duration(packet), Ok(960));
            assert_eq!(dec.decode(Some(packet), &mut out).unwrap(), 960);
            assert_eq!(
                dec.final_ranges(),
                enc.final_ranges(),
                "{channels} channels"
            );
        }
        for ch in 0..channels {
            let snr = snr_db(&pcm, &out, channels, ch, usize::from(head.pre_skip));
            assert!(snr > 20.0, "{channels} channels, channel {ch}: {snr:.1} dB");
        }
    }
}

#[test]
fn the_bitrate_is_shared_between_the_streams_and_the_settings_reach_them_all() {
    let mut enc = MultistreamEncoder::new(6).unwrap();
    // 64 kb/s a channel unless asked.
    assert_eq!(enc.bitrate(), 384_000);
    assert!(enc.set_bitrate(5_000).is_err());
    assert!(enc.set_bitrate(8 * 510_000 + 1).is_err());
    enc.set_bitrate(192_000).unwrap();
    let pcm = tones(960 * 50, 6);
    let mut bytes = 0;
    for chunk in pcm.chunks(960 * 6) {
        let mut packet = vec![];
        enc.encode(chunk, &mut packet).unwrap();
        bytes += packet.len();
    }
    // Constant bitrate by default: a second is the bitrate, give or take the framing.
    assert!((23_000..25_000).contains(&bytes), "{bytes} bytes for 24000");

    enc.set_vbr(true);
    enc.reset();
    let mut silent = vec![];
    enc.encode(&vec![0.0; 960 * 6], &mut silent).unwrap();
    // Four streams of a TOC and two bytes, three of them with a length byte.
    assert_eq!(silent.len(), 4 * 3 + 3);
}

#[test]
fn bad_arguments_are_refused_and_append_nothing() {
    assert!(MultistreamEncoder::new(0).is_err());
    assert!(MultistreamEncoder::new(9).is_err());
    let mut enc = MultistreamEncoder::new(3).unwrap();
    let mut packet = vec![9];
    assert!(enc.encode(&[0.0; 960 * 2], &mut packet).is_err());
    assert_eq!(packet, [9]);
}
