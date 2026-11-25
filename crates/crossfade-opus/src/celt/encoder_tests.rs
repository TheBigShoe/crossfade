use super::*;
use crate::celt::CeltFrame;
use crate::celt::decoder::CeltDecoder;
use crate::range::RangeDecoder;

/// `frames` frames of `channels` interleaved test signal `kind` at 48 kHz.
pub(in crate::celt) fn signal(kind: &str, channels: usize, len: usize) -> Vec<f32> {
    let mut seed = 0x1234_5678u32;
    let mut out = Vec::with_capacity(len * channels);
    for i in 0..len {
        let t = i as f32 / 48_000.0;
        for ch in 0..channels {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = ((seed >> 8) as f32 / (1 << 24) as f32) * 2.0 - 1.0;
            let v = match kind {
                "silence" => 0.0,
                "tone" => {
                    0.5 * libm::sinf(2.0 * std::f32::consts::PI * (440.0 + 110.0 * ch as f32) * t)
                }
                "sweep" => 0.4 * libm::sinf(2.0 * std::f32::consts::PI * (50.0 + 9000.0 * t) * t),
                "noise" => 0.3 * noise,
                "clicks" => {
                    if i % 2400 < 24 {
                        0.9 * noise
                    } else {
                        0.001 * noise
                    }
                }
                "dc" => 0.25,
                "full" => {
                    if (i / 37) % 2 == 0 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                "loud" => 3.0 * libm::sinf(2.0 * std::f32::consts::PI * 1000.0 * t),
                _ => unreachable!("{kind}"),
            };
            out.push(v);
        }
    }
    out
}

/// Encodes `pcm` frame by frame into `bytes`-byte frames (CBR) or at `bitrate` (VBR) and
/// decodes each with the CELT decoder, checking the final range; returns the decoded audio
/// and the frame sizes.
pub(in crate::celt) fn round_trip(
    pcm: &[f32],
    channels: usize,
    frame: &CeltEncodeFrame,
    bytes: usize,
) -> (Vec<f32>, Vec<usize>) {
    round_trip_with(CeltEncoder::new(channels), pcm, channels, frame, bytes)
}

/// [`round_trip`] with an encoder as set up.
pub(in crate::celt) fn round_trip_with(
    mut enc: CeltEncoder,
    pcm: &[f32],
    channels: usize,
    frame: &CeltEncodeFrame,
    bytes: usize,
) -> (Vec<f32>, Vec<usize>) {
    let mut dec = CeltDecoder::new(channels);
    let n = frame.frame_size;
    let mut out = vec![0.0f32; pcm.len()];
    let mut sizes = vec![];
    for (f, chunk) in pcm.chunks_exact(n * channels).enumerate() {
        let mut rc = RangeEncoder::new(bytes);
        let len = enc.encode(chunk, frame, &mut rc).expect("encodes");
        let rng = rc.rng();
        assert_eq!(enc.final_range(), rng);
        let data = rc.done().expect("fits");
        assert_eq!(data.len(), len, "frame {f}");
        let mut rd = RangeDecoder::new(&data);
        let celt = CeltFrame {
            frame_size: n,
            start_band: frame.start_band,
            end_band: frame.end_band,
            stream_channels: frame.stream_channels,
            accumulate: false,
        };
        let at = f * n * channels;
        dec.decode(Some(&mut rd), &celt, &mut out[at..at + n * channels])
            .expect("decodes");
        assert_eq!(dec.final_range(), rng, "frame {f} of {} bytes", data.len());
        sizes.push(len);
    }
    (out, sizes)
}

/// The SNR in dB of `got` against `want` delayed by `delay` samples per channel.
pub(in crate::celt) fn snr(want: &[f32], got: &[f32], channels: usize, delay: usize) -> f64 {
    let mut sig = 0.0f64;
    let mut err = 0.0f64;
    let skip = 2 * 960 * channels;
    for i in skip..got.len() {
        let Some(&w) = want.get(i.wrapping_sub(delay * channels)) else {
            continue;
        };
        sig += f64::from(w) * f64::from(w);
        err += (f64::from(w) - f64::from(got[i])).powi(2);
    }
    10.0 * libm::log10(sig / err.max(1e-20))
}

fn frame(n: usize, channels: usize, end: usize) -> CeltEncodeFrame {
    CeltEncodeFrame {
        frame_size: n,
        start_band: 0,
        end_band: end,
        stream_channels: channels,
        bitrate: None,
        vbr: false,
        constrained_vbr: true,
        silk_info: SilkInfo::default(),
    }
}

#[test]
fn every_frame_decodes_with_the_encoders_final_range() {
    for kind in [
        "silence", "tone", "sweep", "noise", "clicks", "dc", "full", "loud",
    ] {
        for channels in [1, 2] {
            for n in [120, 240, 480, 960] {
                for bytes in [2, 3, 8, 40, 160, 1275] {
                    let pcm = signal(kind, channels, 8 * 960);
                    round_trip(&pcm, channels, &frame(n, channels, 21), bytes);
                }
            }
        }
    }
}

#[test]
fn a_stereo_input_coded_as_mono_and_narrow_bands_decode() {
    let pcm = signal("sweep", 2, 8 * 960);
    for end in [13, 17, 19] {
        let f = CeltEncodeFrame {
            stream_channels: 1,
            ..frame(960, 2, end)
        };
        round_trip(&pcm, 2, &f, 100);
    }
}

#[test]
fn vbr_frames_follow_the_bitrate() {
    for channels in [1, 2] {
        for n in [120, 480, 960] {
            for bitrate in [16_000, 64_000, 160_000] {
                let pcm = signal("noise", channels, 40 * 960);
                let f = CeltEncodeFrame {
                    bitrate: Some(bitrate),
                    vbr: true,
                    ..frame(n, channels, 21)
                };
                let (_, sizes) = round_trip(&pcm, channels, &f, 1275);
                let total: usize = sizes.iter().sum();
                let rate = total as f64 * 8.0 * 48_000.0 / (sizes.len() * n) as f64;
                // Within the constrained VBR's reach of the target, after it settles.
                assert!(
                    (rate / f64::from(bitrate) - 1.0).abs() < 0.25,
                    "{channels} ch, {n}, {bitrate}: {rate}"
                );
            }
        }
    }
}

#[test]
fn what_comes_out_sounds_like_what_went_in() {
    // Measured: 34.7, 37.3 and 27.4 dB. The best alignment is DELAY.
    for (kind, channels, bytes, min_snr) in [
        ("tone", 1, 160, 30.0),
        ("tone", 2, 320, 32.0),
        ("sweep", 1, 200, 23.0),
    ] {
        let pcm = signal(kind, channels, 20 * 960);
        let (out, _) = round_trip(&pcm, channels, &frame(960, channels, 21), bytes);
        let best = (0..400)
            .map(|d| snr(&pcm, &out, channels, d))
            .fold(f64::MIN, f64::max);
        let at = snr(&pcm, &out, channels, DELAY);
        assert!(at > min_snr, "{kind} {channels} ch: {at} dB");
        assert_eq!(at, best, "{kind} {channels} ch");
    }
}

/// A buzz of period `period` samples through a resonance, full scale about 0.5.
fn buzz(period: usize, channels: usize, len: usize) -> Vec<f32> {
    let (mut y1, mut y2) = (0.0f32, 0.0f32);
    let mut out = vec![];
    for i in 0..len {
        let x = if i % period == 0 { 0.08 } else { 0.0 };
        let y = x + 1.8 * y1 - 0.9 * y2;
        y2 = y1;
        y1 = y;
        out.extend(std::iter::repeat_n(y, channels));
    }
    out
}

/// Whether each frame of `pcm` codes the post-filter on.
fn postfilter_flags(pcm: &[f32], channels: usize, n: usize, bytes: usize) -> Vec<bool> {
    let mut enc = CeltEncoder::new(channels);
    let f = frame(n, channels, 21);
    pcm.chunks_exact(n * channels)
        .map(|chunk| {
            let mut rc = RangeEncoder::new(bytes);
            enc.encode(chunk, &f, &mut rc).expect("encodes");
            let data = rc.done().expect("fits");
            let mut rd = RangeDecoder::new(&data);
            !rd.dec_bit_logp(15) && rd.dec_bit_logp(1)
        })
        .collect()
}

#[test]
fn a_periodic_signal_turns_the_post_filter_on() {
    for (period, channels, n) in [(200, 1, 960), (97, 2, 480), (400, 1, 240)] {
        let pcm = buzz(period, channels, 20 * 960);
        let flags = postfilter_flags(&pcm, channels, n, 120 * n / 480);
        let on = flags.iter().skip(4).filter(|&&f| f).count();
        assert!(
            on * 4 >= (flags.len() - 4) * 3,
            "period {period}: {flags:?}"
        );
        let pcm = signal("noise", channels, 20 * 960);
        let flags = postfilter_flags(&pcm, channels, n, 120 * n / 480);
        let on = flags.iter().filter(|&&f| f).count();
        assert!(on * 20 <= flags.len(), "noise: {flags:?}");
    }
}

/// A harmonic tone: `harmonics` partials of `f0` Hz falling 3 dB each, with a little vibrato.
fn harmonic(f0: f32, harmonics: usize, len: usize) -> Vec<f32> {
    let tau = 2.0 * std::f32::consts::PI;
    let mut phase = 0.0f32;
    (0..len)
        .map(|i| {
            let t = i as f32 / 48_000.0;
            phase += tau * f0 * (1.0 + 0.003 * libm::sinf(tau * 5.0 * t)) / 48_000.0;
            let mut v = 0.0;
            let mut a = 0.3f32;
            for h in 1..=harmonics {
                v += a * libm::sinf(h as f32 * phase);
                a *= 0.707;
            }
            v
        })
        .collect()
}

#[test]
fn the_post_filter_codes_a_harmonic_signal_better() {
    // Measured, at 64 kb/s: 27.4 dB with the pre-filter and 20.9 without for 150 Hz, 30.7
    // and 25.7 for 300 Hz. (opus_compare's figure does not always agree on these signals.)
    for f0 in [150.0, 300.0] {
        let pcm = harmonic(f0, 20, 30 * 960);
        let f = frame(960, 1, 21);
        let (with, _) = round_trip(&pcm, 1, &f, 160);
        let mut off = CeltEncoder::new(1);
        off.set_prediction(1);
        let (without, _) = round_trip_with(off, &pcm, 1, &f, 160);
        let with = snr(&pcm, &with, 1, DELAY);
        let without = snr(&pcm, &without, 1, DELAY);
        assert!(with > without + 4.0, "{f0} Hz: {with} vs {without} dB");
    }
}

/// Bits standing for SILK's layer of a hybrid frame: what the test writes before CELT and
/// reads back before decoding it.
const SILK_STAND_IN: [(u32, u32); 4] = [(0x2b, 7), (0x1f3, 9), (0x5, 3), (0x3c, 8)];

/// Codes each frame of `pcm` as the CELT half of a hybrid frame (SILK's bits first), decodes
/// it with the final range checked; returns the frame sizes and the transient flags.
fn hybrid_round_trip(
    enc: &mut CeltEncoder,
    pcm: &[f32],
    channels: usize,
    frame: &CeltEncodeFrame,
    bytes: usize,
) -> (Vec<usize>, Vec<bool>) {
    let mut dec = CeltDecoder::new(channels);
    let n = frame.frame_size;
    let mut out = vec![0.0f32; n * channels];
    let (mut sizes, mut transients) = (vec![], vec![]);
    for chunk in pcm.chunks_exact(n * channels) {
        let mut rc = RangeEncoder::new(bytes);
        for (v, bits) in SILK_STAND_IN {
            rc.encode_bin(v, v + 1, bits);
        }
        let len = enc.encode(chunk, frame, &mut rc).expect("encodes");
        let rng = rc.rng();
        let data = rc.done().expect("fits");
        assert_eq!(data.len(), len);
        let mut rd = RangeDecoder::new(&data);
        for (v, bits) in SILK_STAND_IN {
            assert_eq!(rd.decode_bin(bits), v);
            rd.update(v, v + 1, 1 << bits);
        }
        transients.push(rd.clone().dec_bit_logp(3));
        let celt = CeltFrame {
            frame_size: n,
            start_band: 17,
            end_band: frame.end_band,
            stream_channels: frame.stream_channels,
            accumulate: false,
        };
        dec.decode(Some(&mut rd), &celt, &mut out).expect("decodes");
        assert_eq!(dec.final_range(), rng);
        sizes.push(len);
    }
    (sizes, transients)
}

fn hybrid(n: usize, channels: usize, end: usize, signal_type: i32, offset: i32) -> CeltEncodeFrame {
    CeltEncodeFrame {
        start_band: 17,
        silk_info: SilkInfo {
            signal_type,
            offset,
        },
        ..frame(n, channels, end)
    }
}

#[test]
fn hybrid_frames_decode_with_the_final_range() {
    for kind in ["silence", "tone", "noise", "clicks", "full"] {
        for channels in [1, 2] {
            for n in [480, 960] {
                for end in [19, 21] {
                    for (bytes, bitrate) in [(12, None), (60, None), (1275, Some(16_000))] {
                        let pcm = signal(kind, channels, 6 * 960);
                        let f = CeltEncodeFrame {
                            bitrate,
                            vbr: bitrate.is_some(),
                            constrained_vbr: false,
                            ..hybrid(n, channels, end, 1, 100)
                        };
                        let mut enc = CeltEncoder::new(channels);
                        hybrid_round_trip(&mut enc, &pcm, channels, &f, bytes);
                    }
                }
            }
        }
    }
}

#[test]
fn a_tonal_silk_frame_gives_hybrid_vbr_more_bits_than_a_noisy_one() {
    // libopus moves the target by +12 (offset under 100) or -18 bits (over 100) per 2.5 ms.
    let pcm = signal("noise", 1, 10 * 960);
    let mut sizes = vec![];
    for offset in [60, 100, 140] {
        let f = CeltEncodeFrame {
            bitrate: Some(12_000),
            vbr: true,
            constrained_vbr: false,
            ..hybrid(960, 1, 21, 1, offset)
        };
        let (s, _) = hybrid_round_trip(&mut CeltEncoder::new(1), &pcm, 1, &f, 1275);
        sizes.push(s.iter().sum::<usize>());
    }
    assert!(sizes[0] > sizes[1] && sizes[1] > sizes[2], "{sizes:?}");
}

#[test]
fn low_rate_hybrid_codes_weak_transients_as_long_blocks_unless_voiced() {
    // Below 15 bytes, an onset that masks weakly is no transient in an unvoiced frame
    // (libopus's weak transients); a voiced frame keeps it.
    let mut pcm = vec![0.0f32; 4 * 960];
    let mut seed = 7u32;
    for (i, v) in pcm.iter_mut().enumerate() {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let r = ((seed >> 8) as f32 / (1 << 24) as f32) * 2.0 - 1.0;
        *v = if i >= 2 * 960 + 500 {
            0.2 * r
        } else {
            0.05 * r
        };
    }
    let flags = |signal_type| {
        let f = hybrid(960, 1, 21, signal_type, 100);
        hybrid_round_trip(&mut CeltEncoder::new(1), &pcm, 1, &f, 12).1
    };
    assert_eq!(flags(2)[2], true, "voiced");
    assert_eq!(flags(1)[2], false, "unvoiced");
}
