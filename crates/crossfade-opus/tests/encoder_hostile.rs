//! Any samples at any setting through the public encoder: never a panic, always a packet of
//! the bitrate's size that the decoder reads to the encoder's final range, and the same bytes
//! from the same input.

use crossfade_opus::{Decoder, Encoder, packet_duration};
use proptest::prelude::*;

#[derive(Debug, Clone)]
struct Case {
    channels: usize,
    frame_size: usize,
    bitrate: u32,
    /// Raw bits of each sample: every float there is, NaNs and infinities included.
    samples: Vec<u32>,
    /// Scales the well-formed part of the signal, to far past full scale.
    loud: f32,
}

fn case() -> impl Strategy<Value = Case> {
    (
        1usize..=2,
        prop::sample::select(vec![120usize, 240, 480, 960]),
        6_000u32..=510_000,
        prop::collection::vec(any::<u32>(), 0..64),
        prop::sample::select(vec![0.0f32, 1e-6, 0.5, 1.0, 40.0, 1e30]),
    )
        .prop_map(|(channels, frame_size, bitrate, samples, loud)| Case {
            channels,
            frame_size,
            bitrate,
            samples,
            loud,
        })
}

/// Six frames: a tone scaled by `loud`, with the case's raw samples scattered over it.
fn pcm(c: &Case) -> Vec<f32> {
    let n = c.frame_size * c.channels * 6;
    let mut v: Vec<f32> = (0..n)
        .map(|i| c.loud * ((i / c.channels) as f32 * 0.05).sin())
        .collect();
    for (k, &bits) in c.samples.iter().enumerate() {
        let at = (k * 7919 + bits as usize) % n;
        v[at] = f32::from_bits(bits);
    }
    v
}

fn encode_all(c: &Case) -> Vec<Vec<u8>> {
    let mut enc = Encoder::new(c.channels).unwrap();
    enc.set_bitrate(c.bitrate).unwrap();
    enc.set_frame_size(c.frame_size).unwrap();
    let mut dec = Decoder::new(c.channels).unwrap();
    let mut packets = vec![];
    for chunk in pcm(c).chunks(c.frame_size * c.channels) {
        let mut packet = vec![];
        enc.encode(chunk, &mut packet).expect("any samples encode");
        assert!((3..=1276).contains(&packet.len()));
        assert_eq!(packet_duration(&packet), Ok(c.frame_size));
        let mut out = vec![];
        assert_eq!(dec.decode(Some(&packet), &mut out), Ok(c.frame_size));
        assert_eq!(dec.final_range(), enc.final_range());
        assert!(out.iter().all(|v| v.is_finite()));
        packets.push(packet);
    }
    packets
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn any_samples_encode_to_packets_the_decoder_reads(c in case()) {
        let first = encode_all(&c);
        prop_assert!(first.iter().all(|p| p.len() == first[0].len()), "constant bitrate");
        prop_assert_eq!(encode_all(&c), first, "the same input, the same bytes");
    }
}
