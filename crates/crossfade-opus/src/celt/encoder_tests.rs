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
    let mut enc = CeltEncoder::new(channels);
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
        assert!(on * 4 >= (flags.len() - 4) * 3, "period {period}: {flags:?}");
        let pcm = signal("noise", channels, 20 * 960);
        let flags = postfilter_flags(&pcm, channels, n, 120 * n / 480);
        assert!(flags.iter().all(|&f| !f), "noise: {flags:?}");
    }
}

#[test]
fn the_post_filter_codes_a_periodic_signal_better() {
    let pcm = buzz(240, 1, 30 * 960);
    let (out, _) = round_trip(&pcm, 1, &frame(960, 1, 21), 80);
    // Measured: 13.7 dB without the pre-filter.
    let got = snr(&pcm, &out, 1, DELAY);
    assert!(got > 15.0, "{got} dB");
}
