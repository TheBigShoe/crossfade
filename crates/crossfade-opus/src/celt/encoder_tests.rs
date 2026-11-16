use super::*;
use crate::celt::{CeltDecoder, CeltFrame};
use crate::range::RangeDecoder;
use crate::range_encoder::RangeEncoder;

/// The encoder's look-ahead: its MDCT window reaches one overlap past the frame.
const DELAY: usize = 120;

fn frame(frame_size: usize, channels: usize) -> CeltFrame {
    CeltFrame {
        frame_size,
        start_band: 0,
        end_band: 21,
        stream_channels: channels,
        accumulate: false,
    }
}

/// Encodes `pcm` (interleaved, full scale 1.0) frame by frame into `bytes` each, decodes
/// every frame with [`CeltDecoder`], checks that both ended each frame on the same range, and
/// returns the decoded signal.
fn round_trip(pcm: &[f32], channels: usize, frame_size: usize, bytes: usize) -> Vec<f32> {
    let mut enc = CeltEncoder::new(channels);
    let mut dec = CeltDecoder::new(channels);
    let mut out = Vec::with_capacity(pcm.len());
    for (k, chunk) in pcm.chunks_exact(frame_size * channels).enumerate() {
        let mut renc = RangeEncoder::new(bytes);
        enc.encode(chunk, &frame(frame_size, channels), &mut renc)
            .expect("a frame encodes");
        let rng = renc.rng();
        let data = renc.done().expect("the frame fits its bytes");
        assert_eq!(data.len(), bytes);
        assert_eq!(enc.final_range(), rng);

        let mut rdec = RangeDecoder::new(&data);
        let mut buf = vec![0.0f32; frame_size * channels];
        dec.decode(Some(&mut rdec), &frame(frame_size, channels), &mut buf)
            .expect("our own frame decodes");
        assert_eq!(
            dec.final_range(),
            rng,
            "frame {k}: {channels} ch, {frame_size} samples, {bytes} bytes"
        );
        assert!(buf.iter().all(|v| v.is_finite()));
        out.extend_from_slice(&buf);
    }
    out
}

/// Whether a frame's pitch filter flag is set: the second bit, after the silence flag.
fn pitch_filter_on(data: &[u8]) -> bool {
    let mut dec = RangeDecoder::new(data);
    !dec.dec_bit_logp(15) && dec.dec_bit_logp(1)
}

/// The frames of `pcm` as [`round_trip`] codes them (and checks them).
fn frames_of(pcm: &[f32], frame_size: usize, bytes: usize) -> Vec<Vec<u8>> {
    let mut enc = CeltEncoder::new(1);
    let mut dec = CeltDecoder::new(1);
    pcm.chunks_exact(frame_size)
        .map(|chunk| {
            let mut renc = RangeEncoder::new(bytes);
            enc.encode(chunk, &frame(frame_size, 1), &mut renc).unwrap();
            let rng = renc.rng();
            let data = renc.done().unwrap();
            let mut rdec = RangeDecoder::new(&data);
            let mut buf = vec![0.0f32; frame_size];
            dec.decode(Some(&mut rdec), &frame(frame_size, 1), &mut buf)
                .unwrap();
            assert_eq!(dec.final_range(), rng);
            data
        })
        .collect()
}

#[test]
fn a_pitched_voice_turns_the_pitch_filter_on_and_noise_does_not() {
    // A 150 Hz pulse train with a falling spectrum, as a voiced sound has.
    let voice: Vec<f32> = (0..960 * 20)
        .map(|i| {
            let t = f64::from(i) / 48000.0;
            (1..=30)
                .map(|h| {
                    0.25 / f64::from(h)
                        * libm::sin(std::f64::consts::TAU * 150.0 * f64::from(h) * t)
                })
                .sum::<f64>() as f32
        })
        .collect();
    for frame_size in [240, 960] {
        let frames = frames_of(&voice, frame_size, 60 * frame_size / 960 + 20);
        let on = frames[4..].iter().filter(|f| pitch_filter_on(f)).count();
        assert!(
            on * 10 >= (frames.len() - 4) * 9,
            "{frame_size}: {on} of {}",
            frames.len() - 4
        );
    }
    let mut s = 99u32;
    let noise: Vec<f32> = (0..960 * 20)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((s >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * 0.5
        })
        .collect();
    let frames = frames_of(&noise, 960, 80);
    assert_eq!(frames.iter().filter(|f| pitch_filter_on(f)).count(), 0);
    // Too few bytes to spend any on it.
    assert!(
        frames_of(&voice, 960, 12)
            .iter()
            .all(|f| !pitch_filter_on(f))
    );
}

#[test]
fn the_pitch_filter_makes_a_voice_cleaner_at_the_same_rate() {
    let voice: Vec<f32> = (0..960 * 30)
        .map(|i| {
            let t = f64::from(i) / 48000.0;
            (1..=30)
                .map(|h| {
                    0.25 / f64::from(h)
                        * libm::sin(std::f64::consts::TAU * 150.0 * f64::from(h) * t)
                })
                .sum::<f64>() as f32
        })
        .collect();
    let got = round_trip(&voice, 1, 960, 50);
    let snr = snr_db(&voice, &got, 1, 960 * 6);
    // Without the filter: 13.7 dB.
    assert!(snr > 15.0, "{snr:.1} dB");
}

/// A few partials and a little noise: music-like, deterministic.
fn tones(len: usize, channels: usize) -> Vec<f32> {
    let mut s = 12345u32;
    let mut pcm = Vec::with_capacity(len * channels);
    for i in 0..len {
        let t = i as f64 / 48000.0;
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (f64::from(s >> 9) / f64::from(1u32 << 23)) - 0.5;
        let tau = std::f64::consts::TAU;
        let a = 0.3 * libm::sin(tau * 440.0 * t)
            + 0.15 * libm::sin(tau * 1320.0 * t + 0.4)
            + 0.08 * libm::sin(tau * 5200.0 * t)
            + 0.01 * noise;
        let b = 0.25 * libm::sin(tau * 440.0 * t + 0.9)
            + 0.1 * libm::sin(tau * 2500.0 * t)
            + 0.01 * noise;
        pcm.push(a as f32);
        if channels == 2 {
            pcm.push(b as f32);
        }
    }
    pcm
}

/// Signal-to-noise ratio in dB of `got` against `want` delayed by [`DELAY`] samples, past
/// the first `skip` frames' worth.
fn snr_db(want: &[f32], got: &[f32], channels: usize, skip: usize) -> f64 {
    let (mut sig, mut err) = (0.0f64, 0.0f64);
    for i in skip + DELAY..got.len() / channels {
        for ch in 0..channels {
            let w = f64::from(want[(i - DELAY) * channels + ch]);
            let g = f64::from(got[i * channels + ch]);
            sig += w * w;
            err += (w - g) * (w - g);
        }
    }
    10.0 * libm::log10(sig / err.max(1e-30))
}

#[test]
fn music_survives_at_64_kbps_per_channel_delayed_by_the_overlap() {
    // 20 ms frames of 160 bytes a channel: transparent-ish for a few partials.
    for channels in 1..=2 {
        let pcm = tones(960 * 25, channels);
        let got = round_trip(&pcm, channels, 960, 160 * channels);
        let snr = snr_db(&pcm, &got, channels, 960 * 2);
        assert!(snr > 25.0, "{channels} ch: {snr:.1} dB");
        // The delay is exactly the overlap: one sample either way is far worse.
        let shifted: Vec<f32> = got[channels..].to_vec();
        assert!(snr_db(&pcm, &shifted, channels, 960 * 2) < snr - 3.0);
    }
}

#[test]
fn every_frame_size_and_rate_decodes_to_the_encoders_range() {
    for channels in 1..=2 {
        for frame_size in [120, 240, 480, 960] {
            let pcm = tones(frame_size * 12, channels);
            // From far too few bytes for the frame to the format's most.
            for bytes in [2, 3, 6, 12, 40, 150, 600, 1275] {
                let got = round_trip(&pcm, channels, frame_size, bytes);
                assert_eq!(got.len(), pcm.len());
            }
        }
    }
}

#[test]
fn quality_grows_with_the_rate() {
    let pcm = tones(480 * 40, 1);
    let snr: Vec<f64> = [10, 30, 90, 300]
        .iter()
        .map(|&bytes| snr_db(&pcm, &round_trip(&pcm, 1, 480, bytes), 1, 480 * 4))
        .collect();
    assert!(snr.windows(2).all(|w| w[1] > w[0] + 3.0), "{snr:?}");
    assert!(snr[3] > 40.0, "{snr:?}");
}

#[test]
fn silence_is_flagged_and_decodes_to_exact_silence() {
    let pcm = vec![0.0f32; 960 * 2 * 4];
    let got = round_trip(&pcm, 2, 960, 40);
    assert!(got.iter().all(|&v| v == 0.0));
    // The flag is the frame's first bit, at probability 2^-15: a frame of all ones.
    let mut enc = CeltEncoder::new(1);
    let mut renc = RangeEncoder::new(8);
    enc.encode(&[0.0; 480], &frame(480, 1), &mut renc).unwrap();
    let data = renc.done().unwrap();
    assert_eq!(data[0], 0xFF);
}

#[test]
fn a_click_after_silence_is_coded_as_a_transient_without_pre_echo() {
    // An impulse late in the frame: with one long window its quantisation noise would
    // spread over the 20 ms before it; short blocks keep it near the click.
    let mut pcm = vec![0.0f32; 960 * 6];
    for (i, v) in pcm[960 * 3 + 700..960 * 3 + 760].iter_mut().enumerate() {
        // A 3 kHz burst (alternating samples would be 24 kHz, above every band).
        *v = if (i / 8) % 2 == 0 { 0.8 } else { -0.8 };
    }
    let got = round_trip(&pcm, 1, 960, 80);
    let at = 960 * 3 + 700 + DELAY;
    let before: f32 = got[at - 600..at - 200].iter().map(|v| v * v).sum();
    let around: f32 = got[at..at + 60].iter().map(|v| v * v).sum();
    assert!(around > 5.0, "the click is there: {around}");
    assert!(before < around * 1e-4, "pre-echo {before} against {around}");
}

#[test]
fn identical_channels_cost_no_more_than_mono_to_reach_its_quality() {
    let mono = tones(960 * 20, 1);
    let stereo: Vec<f32> = mono.iter().flat_map(|&v| [v, v]).collect();
    let m = snr_db(&mono, &round_trip(&mono, 1, 960, 100), 1, 960 * 2);
    let s = snr_db(&stereo, &round_trip(&stereo, 2, 960, 110), 2, 960 * 2);
    assert!(s > m - 3.0, "stereo {s:.1} dB against mono {m:.1} dB");
}

#[test]
fn bad_arguments_are_refused_and_hostile_samples_do_not_panic() {
    let mut enc = CeltEncoder::new(2);
    let mut renc = RangeEncoder::new(50);
    assert!(enc.encode(&[0.0; 100], &frame(100, 1), &mut renc).is_err());
    assert!(enc.encode(&[0.0; 959], &frame(960, 1), &mut renc).is_err());
    let mut tiny = RangeEncoder::new(1);
    assert!(enc.encode(&[0.0; 240], &frame(120, 2), &mut tiny).is_err());
    // NaN, infinities and far past full scale: something is coded, and it decodes.
    for bad in [f32::NAN, f32::INFINITY, -1e30, 1e-40] {
        let mut pcm = tones(480 * 4, 2);
        pcm.iter_mut().step_by(7).for_each(|v| *v = bad);
        let got = round_trip(&pcm, 2, 480, 60);
        assert!(got.iter().all(|v| v.is_finite()));
    }
}

#[test]
fn reset_makes_the_encoder_as_new() {
    let pcm = tones(480 * 6, 1);
    let mut a = CeltEncoder::new(1);
    let bytes = |enc: &mut CeltEncoder, chunk: &[f32]| {
        let mut renc = RangeEncoder::new(60);
        enc.encode(chunk, &frame(480, 1), &mut renc).unwrap();
        renc.done().unwrap()
    };
    let first: Vec<Vec<u8>> = pcm.chunks(480).map(|c| bytes(&mut a, c)).collect();
    a.reset();
    let again: Vec<Vec<u8>> = pcm.chunks(480).map(|c| bytes(&mut a, c)).collect();
    assert_eq!(first, again);
}
