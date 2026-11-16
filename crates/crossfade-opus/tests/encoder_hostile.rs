//! Any samples at any setting through the public encoder: never a panic, always a packet of
//! the bitrate's size (or, at a variable bitrate, of its own) that the decoder reads to the encoder's final range, and the same bytes
//! from the same input.

use crossfade_opus::{Decoder, Encoder, packet_duration};
use proptest::prelude::*;

#[derive(Debug, Clone)]
struct Case {
    channels: usize,
    frame_size: usize,
    bitrate: u32,
    vbr: bool,
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
        any::<bool>(),
        prop::collection::vec(any::<u32>(), 0..64),
        prop::sample::select(vec![0.0f32, 1e-6, 0.5, 1.0, 40.0, 1e30]),
    )
        .prop_map(|(channels, frame_size, bitrate, vbr, samples, loud)| Case {
            channels,
            frame_size,
            bitrate,
            vbr,
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
    enc.set_vbr(c.vbr);
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
        prop_assert!(c.vbr || first.iter().all(|p| p.len() == first[0].len()), "constant bitrate");
        prop_assert_eq!(encode_all(&c), first, "the same input, the same bytes");
    }
}

#[derive(Debug, Clone)]
struct SurroundCase {
    channels: usize,
    /// Where in the layout's bitrate range, in thousandths.
    rate: u32,
    vbr: bool,
    samples: Vec<u32>,
}

fn surround_case() -> impl Strategy<Value = SurroundCase> {
    (
        1usize..=8,
        0u32..=1000,
        any::<bool>(),
        prop::collection::vec(any::<u32>(), 0..64),
    )
        .prop_map(|(channels, rate, vbr, samples)| SurroundCase {
            channels,
            rate,
            vbr,
            samples,
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn any_samples_in_any_layout_encode_to_packets_the_multistream_decoder_reads(
        c in surround_case()
    ) {
        use crossfade_opus::{MultistreamDecoder, MultistreamEncoder};
        let mut enc = MultistreamEncoder::new(c.channels).unwrap();
        let range = MultistreamEncoder::bitrate_range(c.channels);
        let span = u64::from(range.end() - range.start());
        let bitrate = range.start() + (span * u64::from(c.rate) / 1000) as u32;
        enc.set_bitrate(bitrate).unwrap();
        enc.set_vbr(c.vbr);
        let head = enc.head(48_000);
        let mut dec = MultistreamDecoder::new(&head.mapping, c.channels).unwrap();
        let n = 960 * c.channels;
        for frame in 0..4 {
            let mut pcm: Vec<f32> = (0..n).map(|i| ((i + frame * n) as f32 * 0.01).sin()).collect();
            for (k, &bits) in c.samples.iter().enumerate() {
                pcm[(k * 7919 + bits as usize + frame) % n] = f32::from_bits(bits);
            }
            let mut packet = vec![];
            enc.encode(&pcm, &mut packet).expect("any samples encode");
            let mut out = vec![];
            prop_assert_eq!(dec.decode(Some(&packet), &mut out), Ok(960));
            prop_assert_eq!(dec.final_ranges(), enc.final_ranges());
            prop_assert!(out.iter().all(|v| v.is_finite()));
        }
    }
}
