//! Hostile CELT packets through the public decoder: every CELT configuration (TOC configs
//! 16-31), mono and stereo, every frame packing, random and mutated frame bytes, in sequences
//! that carry state from packet to packet. Decoding never panics, appends exactly the packet's
//! duration or nothing, and never yields a non-finite sample.

use crossfade_opus::testing::packet_writer::{Packing, write_packet};
use crossfade_opus::{Decoder, packet_duration};
use proptest::prelude::*;

#[derive(Debug, Clone)]
struct Case {
    config: u8,
    stereo: bool,
    packing: u8,
    frames: Vec<Vec<u8>>,
    /// A byte to overwrite in the packet after writing it: (offset, value).
    mutate: Option<(usize, u8)>,
}

fn case() -> impl Strategy<Value = Case> {
    (
        16u8..=31,
        any::<bool>(),
        0u8..5,
        prop::collection::vec(prop::collection::vec(any::<u8>(), 0..160), 1..=6),
        prop::option::of((any::<usize>(), any::<u8>())),
    )
        .prop_map(|(config, stereo, packing, frames, mutate)| Case {
            config,
            stereo,
            packing,
            frames,
            mutate,
        })
}

fn packet(c: &Case) -> Vec<u8> {
    let frames: Vec<&[u8]> = c.frames.iter().map(Vec::as_slice).collect();
    let mut p = match c.packing {
        0 => write_packet(c.config, c.stereo, &frames[..1], Packing::One, false),
        1 => {
            let f = frames[0];
            write_packet(c.config, c.stereo, &[f, f], Packing::TwoEqual, false)
        }
        2 => {
            let second = frames.get(1).copied().unwrap_or(frames[0]);
            write_packet(
                c.config,
                c.stereo,
                &[frames[0], second],
                Packing::TwoDifferent,
                false,
            )
        }
        3 => {
            let same: Vec<&[u8]> = frames.iter().map(|_| frames[0]).collect();
            write_packet(
                c.config,
                c.stereo,
                &same,
                Packing::Signalled {
                    vbr: false,
                    padding: None,
                },
                false,
            )
        }
        _ => write_packet(
            c.config,
            c.stereo,
            &frames,
            Packing::Signalled {
                vbr: true,
                padding: Some(3),
            },
            false,
        ),
    };
    if let Some((at, v)) = c.mutate {
        // Never the TOC: that would leave CELT.
        if p.len() > 1 {
            let at = 1 + at % (p.len() - 1);
            p[at] = v;
        }
    }
    p
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn hostile_celt_packets_decode_to_finite_audio_or_an_error(
        channels in 1usize..=2,
        cases in prop::collection::vec(case(), 1..6),
    ) {
        let mut d = Decoder::new(channels).unwrap();
        for c in &cases {
            let p = packet(c);
            let mut out = vec![0.5f32; 3];
            match d.decode(Some(&p), &mut out) {
                Ok(n) => {
                    prop_assert_eq!(Ok(n), packet_duration(&p));
                    prop_assert_eq!(out.len(), 3 + n * channels);
                    prop_assert!(out.iter().all(|v| v.is_finite()));
                }
                Err(_) => prop_assert_eq!(out, vec![0.5f32; 3]),
            }
        }
    }
}

#[test]
fn every_celt_configuration_decodes_extreme_frames() {
    // Frames of all zeros, all ones and alternating bits, at the smallest and largest sizes,
    // in every configuration, mono and stereo, into one and two output channels.
    for channels in [1, 2] {
        let mut d = Decoder::new(channels).unwrap();
        for config in 16u8..=31 {
            for stereo in [false, true] {
                for fill in [0x00u8, 0xFF, 0x55, 0xAA] {
                    for len in [2usize, 3, 7, 50, 400, 1275] {
                        let frame = vec![fill; len];
                        let p = write_packet(config, stereo, &[&frame], Packing::One, false);
                        let mut out = vec![];
                        let n = d.decode(Some(&p), &mut out).unwrap();
                        assert_eq!(Ok(n), packet_duration(&p));
                        assert!(out.iter().all(|v| v.is_finite()), "{config} {fill} {len}");
                    }
                }
            }
        }
    }
}
