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
