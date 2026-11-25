//! The encoder through the crate's own decoder: every packet decodes to exactly the samples it
//! was given.

use crossfade_alac::{Config, Decoder, Encoder, EncoderConfig, Error};

/// Deterministic test signals, integer or plain IEEE arithmetic only.
mod signal {
    /// A full-scale-ish sinusoid from a resonator (no `sin`, so the same on every platform).
    pub(crate) fn sine(frames: usize, depth: u8, step: f64, level: f64) -> Vec<i32> {
        let max = f64::from((1u32 << (depth - 1)) - 1) * level;
        let c = 2.0 - step * step;
        let (mut a, mut b) = (0.0f64, step);
        (0..frames)
            .map(|_| {
                let next = c * b - a;
                (a, b) = (b, next);
                (a * max).round() as i32
            })
            .collect()
    }

    /// SplitMix64 noise spread over the whole range of `depth` bits.
    pub(crate) fn noise(frames: usize, depth: u8, seed: u64) -> Vec<i32> {
        let mut s = seed;
        (0..frames)
            .map(|_| {
                s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = s;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                z ^= z >> 31;
                ((z as i64) >> (64 - u32::from(depth))) as i32
            })
            .collect()
    }

    /// The most negative and most positive values, in runs and alternating.
    pub(crate) fn extremes(frames: usize, depth: u8) -> Vec<i32> {
        let max = ((1i64 << (depth - 1)) - 1) as i32;
        let min = (-(1i64 << (depth - 1))) as i32;
        (0..frames)
            .map(|i| match (i / 37) % 3 {
                0 => min,
                1 => max,
                _ if i % 2 == 0 => min,
                _ => max,
            })
            .collect()
    }

    /// Channels side by side, interleaved.
    pub(crate) fn interleave(channels: &[Vec<i32>]) -> Vec<i32> {
        let frames = channels[0].len();
        (0..frames)
            .flat_map(|i| channels.iter().map(move |c| c[i]))
            .collect()
    }
}

fn config(depth: u8, channels: u8, frame_length: u32) -> EncoderConfig {
    let mut c = EncoderConfig::new(44100, channels, depth);
    c.frame_length = frame_length;
    c
}

/// Encodes `pcm` packet by packet, decodes every packet, and checks the samples come back.
fn round_trip(c: EncoderConfig, pcm: &[i32]) -> (Vec<Vec<u8>>, Config) {
    let mut enc = Encoder::new(c).unwrap();
    let per_packet = c.frame_length as usize * usize::from(c.channels);
    let mut packets = Vec::new();
    for chunk in pcm.chunks(per_packet) {
        let mut out = vec![1, 2, 3];
        enc.encode(chunk, &mut out).unwrap();
        packets.push(out);
    }
    let cookie = enc.config();
    let mut dec = Decoder::new(Config::parse(&cookie.write()).unwrap()).unwrap();
    let (mut back, mut buf) = (Vec::new(), Vec::new());
    for (i, p) in packets.iter().enumerate() {
        dec.decode(p, &mut buf)
            .unwrap_or_else(|e| panic!("packet {i}: {e}"));
        back.extend_from_slice(&buf);
    }
    assert!(back == pcm, "{c:?}: decoded samples differ");
    (packets, cookie)
}

/// One signal per channel, each a little different, so channels are not all alike.
fn signals(depth: u8, channels: u8, frames: usize) -> Vec<(&'static str, Vec<i32>)> {
    let per_channel = |f: &dyn Fn(usize) -> Vec<i32>| {
        signal::interleave(&(0..usize::from(channels)).map(f).collect::<Vec<_>>())
    };
    vec![
        ("silence", vec![0; frames * usize::from(channels)]),
        (
            "sine",
            per_channel(&|c| signal::sine(frames, depth, 0.01 + 0.003 * c as f64, 0.9)),
        ),
        (
            "noise",
            per_channel(&|c| signal::noise(frames, depth, c as u64)),
        ),
        (
            "extremes",
            per_channel(&|_| signal::extremes(frames, depth)),
        ),
    ]
}

#[test]
fn every_depth_and_channel_count_round_trips() {
    for depth in [16, 20, 24, 32] {
        for channels in 1..=8 {
            // Two whole packets and a short last one.
            for (name, pcm) in signals(depth, channels, 2 * 1024 + 333) {
                let c = config(depth, channels, 1024);
                let (packets, _) = round_trip(c, &pcm);
                assert_eq!(packets.len(), 3, "{name}");
            }
        }
    }
}

#[test]
fn other_frame_lengths_round_trip() {
    for frame_length in [1, 2, 3, 5, 16, 31, 33, 100, 1152, 4096, 4097] {
        for (depth, channels) in [(16, 2), (24, 1), (32, 2), (20, 3)] {
            for (name, pcm) in signals(depth, channels, 5000) {
                let (packets, _) = round_trip(config(depth, channels, frame_length), &pcm);
                assert_eq!(
                    packets.len(),
                    5000usize.div_ceil(frame_length as usize),
                    "{name}"
                );
            }
        }
    }
}

#[test]
fn the_default_frame_length_is_apples() {
    let c = EncoderConfig::new(48000, 2, 24);
    assert_eq!(
        (c.sample_rate, c.channels, c.bit_depth, c.frame_length),
        (48000, 2, 24, 4096)
    );
}

#[test]
fn the_cookie_describes_the_stream_as_apple_writes_it() {
    let pcm = signal::interleave(&[
        signal::sine(10_007, 16, 0.02, 0.5),
        signal::noise(10_007, 16, 9),
    ]);
    let (packets, cookie) = round_trip(EncoderConfig::new(44100, 2, 16), &pcm);
    let total: usize = packets.iter().map(Vec::len).sum();
    let max = packets.iter().map(Vec::len).max().unwrap();
    // As afconvert: the bit rate counts every packet as a whole frame long.
    let avg = (total as u64 * 8 * 44100 / (packets.len() as u64 * 4096)) as u32;
    assert_eq!(
        cookie,
        Config {
            frame_length: 4096,
            compatible_version: 0,
            bit_depth: 16,
            pb: 40,
            mb: 10,
            kb: 14,
            channels: 2,
            max_run: 255,
            max_frame_bytes: max as u32,
            avg_bit_rate: avg,
            sample_rate: 44100,
        }
    );
    // Before any packet there is nothing to count.
    let fresh = Encoder::new(EncoderConfig::new(8000, 1, 20))
        .unwrap()
        .config();
    assert_eq!(
        (fresh.max_frame_bytes, fresh.avg_bit_rate, fresh.sample_rate),
        (0, 0, 8000)
    );
}

#[test]
fn bad_configs_are_refused() {
    let bad = |f: fn(&mut EncoderConfig)| {
        let mut c = EncoderConfig::new(44100, 2, 16);
        f(&mut c);
        Encoder::new(c).err()
    };
    let invalid = |what| Some(Error::InvalidConfig(what));
    for (f, what) in [
        (
            (|c: &mut EncoderConfig| c.channels = 0) as fn(&mut EncoderConfig),
            "channel count is not between 1 and 8",
        ),
        (|c| c.channels = 9, "channel count is not between 1 and 8"),
        (|c| c.bit_depth = 8, "bit depth is not 16, 20, 24 or 32"),
        (|c| c.bit_depth = 17, "bit depth is not 16, 20, 24 or 32"),
        (
            |c| c.frame_length = 0,
            "frame length is not between 1 and 1048576",
        ),
        (
            |c| c.frame_length = (1 << 20) + 1,
            "frame length is not between 1 and 1048576",
        ),
        (|c| c.sample_rate = 0, "sample rate is 0"),
    ] {
        assert_eq!(bad(f), invalid(what));
    }
}

#[test]
fn bad_packets_are_refused_and_change_nothing() {
    let mut enc = Encoder::new(config(16, 2, 4)).unwrap();
    let mut out = vec![9];
    let input = |what| Err(Error::InvalidInput(what));
    assert_eq!(enc.encode(&[], &mut out), input("no samples"));
    assert_eq!(
        enc.encode(&[1, 2, 3], &mut out),
        input("sample count is not a multiple of the channel count")
    );
    assert_eq!(
        enc.encode(&[0; 10], &mut out),
        input("more samples than one frame length")
    );
    assert_eq!(
        enc.encode(&[0, 32768], &mut out),
        input("sample out of range for the bit depth")
    );
    assert_eq!(
        enc.encode(&[-32769, 0], &mut out),
        input("sample out of range for the bit depth")
    );
    assert!(out.is_empty(), "a refused packet leaves no bytes");
    assert_eq!(enc.config().max_frame_bytes, 0, "nor any count");

    let mut wide = Encoder::new(config(20, 1, 4)).unwrap();
    assert_eq!(
        wide.encode(&[1 << 19], &mut out),
        input("sample out of range for the bit depth")
    );
    assert_eq!(wide.encode(&[-(1 << 19), (1 << 19) - 1], &mut out), Ok(()));
    // Only the last packet may be short.
    assert_eq!(
        wide.encode(&[0; 4], &mut out),
        input("a packet after a short one")
    );
    let mut full = Encoder::new(config(32, 1, 2)).unwrap();
    assert_eq!(full.encode(&[i32::MIN, i32::MAX], &mut out), Ok(()));
}

/// The bytes of a packet of uncompressed elements: per element a tag, an instance tag, the
/// header and a short frame's length, then every sample at the full depth; END, and padding.
fn escape_bytes(depth: u8, channels: u8, frames: usize, partial: bool) -> usize {
    let elements = [1, 1, 2, 3, 3, 4, 5, 5][usize::from(channels) - 1];
    let header = 3 + 4 + 16 + if partial { 32 } else { 0 };
    let bits = elements * header + frames * usize::from(channels) * usize::from(depth) + 3;
    bits.div_ceil(8)
}

#[test]
fn no_packet_is_larger_than_its_uncompressed_form() {
    for depth in [16, 20, 24, 32] {
        for channels in 1..=8 {
            for (name, pcm) in signals(depth, channels, 1024 + 100) {
                let (packets, _) = round_trip(config(depth, channels, 1024), &pcm);
                let raw = [
                    escape_bytes(depth, channels, 1024, false),
                    escape_bytes(depth, channels, 100, true),
                ];
                for (p, raw) in packets.iter().zip(raw) {
                    assert!(p.len() <= raw, "{name} {depth} {channels}: {}", p.len());
                    if name == "noise" {
                        assert_eq!(p.len(), raw, "{name} {depth} {channels}");
                    }
                }
            }
        }
    }
}

#[test]
fn silence_codes_as_a_run_of_zeros() {
    for depth in [16, 20, 24, 32] {
        let (packets, _) = round_trip(config(depth, 1, 4096), &[0; 4096]);
        assert!(packets[0].len() <= 24, "{depth}: {}", packets[0].len());
    }
}

#[test]
fn a_single_channel_compresses() {
    // The share of the raw size a smooth sine takes; the low bits of 24- and 32-bit samples
    // are mostly passed through.
    for (depth, share) in [(16, 0.25), (20, 0.4), (24, 0.5), (32, 0.75)] {
        let pcm = signal::sine(4096 * 3, depth, 0.013, 0.9);
        let (packets, _) = round_trip(config(depth, 1, 4096), &pcm);
        let raw = escape_bytes(depth, 1, 4096, false) as f64;
        for p in &packets {
            assert!(
                (p.len() as f64) < raw * share,
                "{depth}: {} of {raw}",
                p.len()
            );
        }
    }
}

#[test]
fn channel_pairs_compress() {
    for (depth, share) in [(16, 0.25), (20, 0.4), (24, 0.5), (32, 0.75)] {
        for channels in [2, 6, 8] {
            let pcm = signal::interleave(
                &(0..channels)
                    .map(|c| signal::sine(4096 * 2, depth, 0.011 + 0.002 * f64::from(c), 0.9))
                    .collect::<Vec<_>>(),
            );
            let (packets, _) = round_trip(config(depth, channels, 4096), &pcm);
            let raw = escape_bytes(depth, channels, 4096, false) as f64;
            for p in &packets {
                assert!(
                    (p.len() as f64) < raw * share,
                    "{depth} {channels}: {} of {raw}",
                    p.len()
                );
            }
        }
    }
}

#[test]
fn correlated_stereo_codes_smaller_than_its_channels_apart() {
    let frames = 4096 * 3;
    let left: Vec<i32> = signal::sine(frames, 16, 0.02, 0.9)
        .iter()
        .zip(signal::noise(frames, 8, 1))
        .map(|(s, n)| s + n)
        .collect();
    let right: Vec<i32> = left
        .iter()
        .zip(signal::noise(frames, 4, 2))
        .map(|(s, n)| s + n)
        .collect();
    let bytes = |channels, pcm: &[i32]| -> usize {
        let (packets, _) = round_trip(config(16, channels, 4096), pcm);
        packets.iter().map(Vec::len).sum()
    };
    let apart = bytes(1, &left) + bytes(1, &right);
    let together = bytes(2, &signal::interleave(&[left, right]));
    assert!(
        (together as f64) < apart as f64 * 0.85,
        "{together} against {apart}"
    );
}
