use super::*;

fn frame(frame_size: usize, stream_channels: usize) -> CeltFrame {
    CeltFrame {
        frame_size,
        start_band: 0,
        end_band: 21,
        stream_channels,
        accumulate: false,
    }
}

#[test]
fn a_silence_frame_decodes_to_silence() {
    // The silence flag (logp 15) is set by a frame whose first bits are all ones.
    for cc in [1, 2] {
        let mut d = CeltDecoder::new(cc);
        let data = [0xFF, 0xFF];
        let mut dec = RangeDecoder::new(&data);
        let mut out = vec![1.0; 960 * cc];
        d.decode(Some(&mut dec), &frame(960, 1), &mut out).unwrap();
        assert!(out.iter().all(|&v| v == 0.0), "{cc}");
    }
}

#[test]
fn bad_arguments_are_refused() {
    let mut d = CeltDecoder::new(2);
    let data = [0u8; 8];
    let mut out = vec![0.0; 960 * 2];
    for f in [
        frame(100, 1),
        CeltFrame {
            start_band: 21,
            ..frame(960, 1)
        },
        CeltFrame {
            end_band: 22,
            ..frame(960, 1)
        },
        CeltFrame {
            start_band: 17,
            end_band: 17,
            ..frame(960, 1)
        },
        frame(960, 3),
    ] {
        let mut dec = RangeDecoder::new(&data);
        assert!(
            matches!(
                d.decode(Some(&mut dec), &f, &mut out),
                Err(Error::BadArgument(_))
            ),
            "{f:?}"
        );
    }
    let mut dec = RangeDecoder::new(&data);
    assert!(
        d.decode(Some(&mut dec), &frame(960, 1), &mut out[..10])
            .is_err()
    );
}

#[test]
fn every_frame_size_decodes_any_bytes_to_finite_output() {
    for cc in [1, 2] {
        for c in [1, 2] {
            let mut d = CeltDecoder::new(cc);
            for (k, size) in [120, 240, 480, 960].into_iter().enumerate() {
                for len in [2usize, 3, 10, 60, 200, 1275] {
                    let data: Vec<u8> = (0..len)
                        .map(|i| (i * 131 + k * 7 + len) as u8 ^ 0x5C)
                        .collect();
                    let mut dec = RangeDecoder::new(&data);
                    let mut out = vec![0.0; size * cc];
                    let _ = d.decode(Some(&mut dec), &frame(size, c), &mut out);
                    assert!(out.iter().all(|v| v.is_finite()));
                }
            }
        }
    }
}

/// Frames of pseudo-random bytes, as a stand-in for real ones: they decode to a busy
/// spectrum whatever the bits say.
fn noise_frame(seed: u32, len: usize) -> Vec<u8> {
    let mut s = seed;
    (0..len)
        .map(|_| {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (s >> 16) as u8
        })
        .collect()
}

fn decode_bytes(d: &mut CeltDecoder, data: &[u8], f: &CeltFrame) -> (Result<()>, Vec<f32>, u32) {
    let mut dec = RangeDecoder::new(data);
    let mut out = vec![0.0; f.frame_size * d.channels];
    let r = d.decode(Some(&mut dec), f, &mut out);
    (r, out, dec.rng())
}

fn conceal(d: &mut CeltDecoder, f: &CeltFrame) -> Vec<f32> {
    let mut out = vec![0.0; f.frame_size * d.channels];
    d.decode(None, f, &mut out).unwrap();
    out
}

#[test]
fn concealment_has_the_frame_length_and_stays_bounded() {
    // After real audio, ten losses in a row at every frame size: the pitch-based path for
    // the first five, the noise-based one after; for start band 17 always the noise.
    for cc in [1, 2] {
        for start_band in [0, 17] {
            for size in [120, 240, 480, 960] {
                let f = CeltFrame {
                    start_band,
                    ..frame(size, cc)
                };
                let mut d = CeltDecoder::new(cc);
                let mut peak = 0.0f32;
                for k in 0..4 {
                    let (_, out, _) = decode_bytes(&mut d, &noise_frame(k, 120), &f);
                    peak = out.iter().fold(peak, |p, v| p.max(v.abs()));
                }
                for _ in 0..10 {
                    let out = conceal(&mut d, &f);
                    assert_eq!(out.len(), size * cc);
                    assert!(out.iter().all(|v| v.is_finite() && v.abs() <= 4.0 * peak.max(0.01)));
                }
            }
        }
    }
}

#[test]
fn concealment_of_silence_is_silence() {
    // A fresh decoder conceals zeros while the pitch-based path runs; after decoded silence
    // the noise-based path's background energy has fallen to nothing as well.
    let f = frame(960, 1);
    let mut d = CeltDecoder::new(2);
    for _ in 0..5 {
        assert!(conceal(&mut d, &f).iter().all(|&v| v == 0.0));
    }
    let mut d = CeltDecoder::new(2);
    for _ in 0..50 {
        let (r, out, _) = decode_bytes(&mut d, &[0xFF, 0xFF], &f);
        r.unwrap();
        assert!(out.iter().all(|&v| v == 0.0));
    }
    for _ in 0..20 {
        assert!(conceal(&mut d, &f).iter().all(|v| v.abs() < 1e-9));
    }
}

#[test]
fn a_frame_after_concealment_decodes_and_reads_the_same_bits() {
    // What a frame reads never depends on the decoder's state, so its final range after a
    // concealed frame is the one it has without the loss.
    for cc in [1, 2] {
        for c in [1, 2] {
            for size in [120, 480, 960] {
                let f = frame(size, c);
                let frames: Vec<Vec<u8>> = (0..4).map(|k| noise_frame(100 + k, 90)).collect();
                let mut clean = CeltDecoder::new(cc);
                let mut lossy = CeltDecoder::new(cc);
                for (k, data) in frames.iter().enumerate() {
                    let (_, _, want) = decode_bytes(&mut clean, data, &f);
                    if k == 1 {
                        conceal(&mut lossy, &f);
                        continue;
                    }
                    let (r, out, got) = decode_bytes(&mut lossy, data, &f);
                    assert!(r.is_ok() && out.iter().all(|v| v.is_finite()));
                    assert_eq!(got, want);
                }
            }
        }
    }
}

#[test]
fn concealment_from_any_state_never_panics() {
    // Losses interleaved with frames of every size and channel count, starting and ending
    // bands changing as a top level may change them.
    let mut d = CeltDecoder::new(2);
    let mut k = 0u32;
    for size in [960, 120, 480, 240] {
        for c in [2, 1] {
            for (start_band, end_band) in [(0, 21), (17, 21), (0, 13), (17, 19)] {
                let f = CeltFrame {
                    start_band,
                    end_band,
                    ..frame(size, c)
                };
                k += 1;
                let _ = decode_bytes(&mut d, &noise_frame(k, 40 + k as usize), &f);
                for _ in 0..(k % 7) {
                    assert!(conceal(&mut d, &f).iter().all(|v| v.is_finite()));
                }
            }
        }
    }
    d.reset();
    assert!(conceal(&mut d, &frame(960, 2)).iter().all(|&v| v == 0.0));
}
