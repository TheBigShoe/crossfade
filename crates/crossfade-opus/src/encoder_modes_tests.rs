//! The encoder's SILK-only, hybrid and CELT-only modes together: the decisions, the framing of
//! long packets, the switches between modes and bandwidths, FEC, DTX and the rate.

use super::tests::{config, digest, encode_and_decode, recipe_frame, signal};
use super::*;
use crate::{Decoder, Packet};

/// `seconds` of something like speech, full scale about 0.3: syllables of a voiced vowel (a
/// pulse train gliding between 90 and 190 Hz through three formants), a fricative (noise
/// tilted up) and a pause, with a longer pause each second, over a quiet background. The
/// second channel, if any, is
/// the first a little quieter and with some noise of its own.
pub(crate) fn speech(channels: usize, seconds: f32) -> Vec<f32> {
    const VOWELS: [(f32, f32, f32); 5] = [
        (730.0, 1090.0, 2440.0),
        (270.0, 2290.0, 3010.0),
        (530.0, 1840.0, 2480.0),
        (570.0, 840.0, 2410.0),
        (300.0, 870.0, 2240.0),
    ];
    let len = (48_000.0 * seconds) as usize;
    let tau = 2.0 * std::f32::consts::PI;
    let mut seed = 0x2545_f491u32;
    let mut rand = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((seed >> 8) as f32 / (1 << 24) as f32) * 2.0 - 1.0
    };
    let mut phase = 0.0f32;
    let mut res = [[0.0f32; 2]; 3];
    let mut last_noise = 0.0f32;
    let mut out = Vec::with_capacity(len * channels);
    for i in 0..len {
        let t = i as f32 / 48_000.0;
        let syl = (t / 0.25) as usize;
        let pos = t - syl as f32 * 0.25;
        let in_pause = t % 1.0 > 0.85;
        let f0 = 140.0 + 40.0 * libm::sinf(tau * 0.7 * t) + 10.0 * (syl % 3) as f32;
        phase += f0 / 48_000.0;
        let pulse = if phase >= 1.0 {
            phase -= 1.0;
            1.0
        } else {
            0.0
        };
        let noise = rand();
        let mut v = 0.0f32;
        if !in_pause && pos < 0.17 {
            // A vowel, faded in and out over 20 ms.
            let env = (pos / 0.02).min(1.0) * ((0.17 - pos) / 0.02).min(1.0);
            let (f1, f2, f3) = VOWELS[syl % VOWELS.len()];
            let x = env * pulse;
            for (k, (f, bw)) in [(f1, 80.0f32), (f2, 100.0), (f3, 150.0)]
                .into_iter()
                .enumerate()
            {
                let r = libm::expf(-std::f32::consts::PI * bw / 48_000.0);
                let c = 2.0 * r * libm::cosf(tau * f / 48_000.0);
                let y = x + c * res[k][0] - r * r * res[k][1];
                res[k][1] = res[k][0];
                res[k][0] = y;
                v += y * [0.06f32, 0.03, 0.015][k];
            }
        } else if !in_pause && pos < 0.22 {
            // A fricative.
            v = 0.08 * (noise - 0.7 * last_noise);
        }
        last_noise = noise;
        // A room's background.
        v += 0.001 * rand();
        out.push(v);
        if channels == 2 {
            out.push(0.8 * v + 0.002 * rand());
        }
    }
    out
}

/// The mode and bandwidth of each packet.
pub(super) fn modes(packets: &[Vec<u8>]) -> Vec<(Mode, Bandwidth)> {
    packets
        .iter()
        .map(|p| {
            let toc = Packet::parse(p).expect("parses").toc();
            (toc.mode(), toc.bandwidth())
        })
        .collect()
}

/// The mode and bandwidth most packets of the last half second have.
pub(super) fn steady(cfg: &EncoderConfig, pcm: &[f32]) -> (Mode, Bandwidth) {
    let (packets, _) = encode_and_decode(cfg, pcm);
    let per_second = 48_000 / cfg.frame_duration.samples();
    let tail = modes(&packets[packets.len() - per_second / 2..]);
    let mut best = (tail[0], 0);
    for m in &tail {
        let n = tail.iter().filter(|x| *x == m).count();
        if n > best.1 {
            best = (*m, n);
        }
    }
    best.0
}

#[test]
fn the_mode_and_bandwidth_follow_the_bitrate_and_the_signal() {
    use Bandwidth::*;
    use Mode::*;
    // On this speech the analysis finds voice (Auto), as libopus's does: SILK and hybrid up
    // to and past 24 kb/s mono and 32 kb/s stereo, as with Voice, which keeps SILK and
    // hybrid to about 64 kb/s; Music goes CELT from 10 kb/s.
    let cases: [(u8, Signal, u32, (Mode, Bandwidth)); 17] = [
        (1, Signal::Auto, 6_000, (Silk, Narrow)),
        (1, Signal::Auto, 9_000, (Silk, Narrow)),
        (1, Signal::Auto, 12_000, (Silk, Wide)),
        (1, Signal::Auto, 16_000, (Hybrid, Full)),
        (1, Signal::Auto, 24_000, (Hybrid, Full)),
        (1, Signal::Auto, 64_000, (Celt, Full)),
        (1, Signal::Voice, 12_000, (Silk, Wide)),
        (1, Signal::Voice, 24_000, (Hybrid, Full)),
        (1, Signal::Voice, 48_000, (Hybrid, Full)),
        (1, Signal::Voice, 96_000, (Celt, Full)),
        (1, Signal::Music, 12_000, (Celt, SuperWide)),
        (2, Signal::Auto, 12_000, (Silk, Wide)),
        (2, Signal::Auto, 16_000, (Hybrid, Full)),
        (2, Signal::Auto, 32_000, (Hybrid, Full)),
        (2, Signal::Voice, 32_000, (Hybrid, Full)),
        (2, Signal::Voice, 64_000, (Celt, Full)),
        (2, Signal::Music, 16_000, (Celt, Full)),
    ];
    let mut got = vec![];
    for (channels, signal_hint, bitrate, _) in cases {
        let pcm = speech(usize::from(channels), 2.0);
        let mut cfg = config(channels, bitrate, true, FrameDuration::Ms20);
        cfg.signal = signal_hint;
        got.push((channels, signal_hint, bitrate, steady(&cfg, &pcm)));
    }
    assert_eq!(got, cases);
}

/// The mode a packet takes when `mode` is forced at `bw`, as libopus maps it.
fn forced_mode(mode: Mode, bw: Bandwidth, frame_size: usize) -> Mode {
    if frame_size < 480 {
        return Mode::Celt;
    }
    match mode {
        Mode::Silk | Mode::Hybrid if bw > Bandwidth::Wide => Mode::Hybrid,
        Mode::Silk | Mode::Hybrid => Mode::Silk,
        Mode::Celt => Mode::Celt,
    }
}

#[test]
fn forced_modes_and_bandwidths_decode_with_the_final_range() {
    use Bandwidth::*;
    for mode in [Mode::Silk, Mode::Hybrid, Mode::Celt] {
        for bw in [Narrow, Medium, Wide, SuperWide, Full] {
            for d in [
                FrameDuration::Ms5,
                FrameDuration::Ms10,
                FrameDuration::Ms20,
                FrameDuration::Ms40,
                FrameDuration::Ms60,
            ] {
                for channels in [1u8, 2] {
                    for (bitrate, vbr) in [(10_000, false), (40_000, true), (40_000, false)] {
                        let mut cfg = config(channels, bitrate, vbr, d);
                        cfg.mode = Some(mode);
                        cfg.bandwidth = Some(bw);
                        let pcm = speech(usize::from(channels), 0.12);
                        let (packets, _) = encode_and_decode(&cfg, &pcm);
                        let mut want_bw = bw;
                        let want = forced_mode(mode, bw, d.samples());
                        if want == Mode::Celt && bw == Medium {
                            want_bw = Wide;
                        }
                        for p in packets.iter().filter(|p| p.len() > 3) {
                            let toc = Packet::parse(p).expect("parses").toc();
                            let what = format!("{mode:?} {bw:?} {d:?} {channels} {bitrate}");
                            // Hybrid needs 15 kb/s and more: below, SILK at wideband.
                            if want == Mode::Hybrid && bitrate < 15_000 {
                                assert_eq!(toc.mode(), Mode::Silk, "{what}");
                                continue;
                            }
                            assert_eq!(toc.mode(), want, "{what}");
                            if want != Mode::Silk {
                                assert_eq!(toc.bandwidth(), want_bw, "{what}");
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn long_packets_hold_one_silk_frame_or_several_20_ms_frames() {
    for (mode, bw) in [
        (Mode::Silk, Bandwidth::Wide),
        (Mode::Hybrid, Bandwidth::Full),
        (Mode::Celt, Bandwidth::Full),
    ] {
        for (d, n) in [(FrameDuration::Ms40, 1920), (FrameDuration::Ms60, 2880)] {
            for vbr in [true, false] {
                let mut cfg = config(1, 32_000, vbr, d);
                cfg.mode = Some(mode);
                cfg.bandwidth = Some(bw);
                let pcm = speech(1, 0.48);
                let (packets, out) = encode_and_decode(&cfg, &pcm);
                assert_eq!(out.len(), pcm.len());
                for p in &packets {
                    let parsed = Packet::parse(p).expect("parses");
                    assert_eq!(parsed.duration(), n);
                    let frames = parsed.frames().len();
                    if mode == Mode::Silk {
                        assert_eq!((frames, parsed.toc().frame_samples()), (1, n));
                    } else {
                        assert_eq!((frames, parsed.toc().frame_samples()), (n / 960, 960));
                    }
                }
            }
        }
    }
}

#[test]
fn cbr_packets_are_the_same_size_in_every_mode() {
    for mode in [Mode::Silk, Mode::Hybrid, Mode::Celt] {
        for (d, fps) in [
            (FrameDuration::Ms10, 100),
            (FrameDuration::Ms20, 50),
            (FrameDuration::Ms40, 25),
            (FrameDuration::Ms60, 50 / 3),
        ] {
            for channels in [1u8, 2] {
                for bitrate in [8_000u32, 16_000, 24_000, 48_000, 96_000] {
                    let mut cfg = config(channels, bitrate, false, d);
                    cfg.mode = Some(mode);
                    let pcm = speech(usize::from(channels), 0.36);
                    let (packets, _) = encode_and_decode(&cfg, &pcm);
                    let n = d.samples() as u32;
                    // 12 times the frame rate, so that the division is exact, and at most a
                    // frame's 1276 bytes whatever the duration (as libopus).
                    let rate12 = 12 * 48_000 / n;
                    let want = ((12 * bitrate / 8 + rate12 / 2) / rate12).min(1276) as usize;
                    let _ = fps;
                    for p in &packets {
                        assert_eq!(p.len(), want, "{mode:?} {d:?} {channels} {bitrate}");
                    }
                }
            }
        }
    }
}

/// Encodes `pcm` with the bitrate of each packet from `schedule` (cycled), decoding each with
/// the final range checked; returns the packets.
fn encode_with_schedule(cfg: &EncoderConfig, pcm: &[f32], schedule: &[u32]) -> Vec<Vec<u8>> {
    let channels = usize::from(cfg.channels);
    let mut enc = Encoder::new(cfg).expect("valid");
    let mut dec = Decoder::new(channels).expect("decoder");
    let n = enc.frame_size();
    let mut packets = vec![];
    let mut out = vec![];
    for (i, frame) in pcm.chunks_exact(n * channels).enumerate() {
        enc.set_bitrate(schedule[i % schedule.len()])
            .expect("a bitrate");
        let mut p = vec![];
        enc.encode(frame, &mut p).expect("encodes");
        dec.decode(Some(&p), &mut out).expect("decodes");
        assert_eq!(dec.final_range(), enc.final_range(), "packet {i}: {cfg:?}");
        packets.push(p);
    }
    packets
}

#[test]
fn sweeping_the_bitrate_crosses_every_mode_and_bandwidth() {
    // Up from 6 to 80 kb/s and down again, a step each packet. SILK widens its band slowly,
    // so a quick sweep may skip hybrid; every mode and bandwidth shows somewhere.
    let schedule: Vec<u32> = (0..60)
        .chain((0..60).rev())
        .map(|k| 6_000 + k * 1_250)
        .collect();
    let mut all = std::collections::HashSet::new();
    for channels in [1u8, 2] {
        for signal_hint in [Signal::Auto, Signal::Voice] {
            for d in [
                FrameDuration::Ms10,
                FrameDuration::Ms20,
                FrameDuration::Ms60,
            ] {
                for vbr in [true, false] {
                    let mut cfg = config(channels, 6_000, vbr, d);
                    cfg.signal = signal_hint;
                    let seconds = (schedule.len() * d.samples()) as f32 / 48_000.0;
                    let pcm = speech(usize::from(channels), seconds);
                    let packets = encode_with_schedule(&cfg, &pcm, &schedule);
                    let seen = modes(&packets);
                    let what = format!("{channels} {signal_hint:?} {d:?} {vbr}");
                    for m in [Mode::Silk, Mode::Celt] {
                        assert!(seen.iter().any(|s| s.0 == m), "{what}: no {m:?}");
                    }
                    let switches = seen.windows(2).filter(|w| w[0] != w[1]).count();
                    assert!(switches >= 2, "{what}: {switches} switches");
                    all.extend(seen);
                }
            }
        }
    }
    for want in [
        (Mode::Silk, Bandwidth::Narrow),
        (Mode::Silk, Bandwidth::Medium),
        (Mode::Silk, Bandwidth::Wide),
        (Mode::Hybrid, Bandwidth::Full),
        (Mode::Celt, Bandwidth::Full),
    ] {
        assert!(all.contains(&want), "{want:?} in {all:?}");
    }
}

/// The largest second difference of `x` (one channel of `channels`) over `range`: a click
/// stands out against a tone's.
fn roughness(x: &[f32], channels: usize, range: std::ops::Range<usize>) -> f32 {
    range
        .map(|i| {
            let s = |k: usize| x[k * channels];
            (s(i) - 2.0 * s(i - 1) + s(i - 2)).abs()
        })
        .fold(0.0, f32::max)
}

#[test]
fn a_mode_switch_on_a_steady_tone_does_not_click() {
    // A tone through CELT, then SILK or hybrid, then CELT again (forced at 20 ms), against the
    // same tone left in one mode: the decoded second difference near each switch, as a
    // multiple of the steady encodes'. Measured: 2.1 (to SILK) and 2.3 (to hybrid), then 0.9
    // back to CELT, which without its redundant frame is 10.
    let tau = 2.0 * std::f32::consts::PI;
    let pcm: Vec<f32> = (0..48_000)
        .map(|i| 0.3 * libm::sinf(tau * 440.0 * i as f32 / 48_000.0))
        .collect();
    for (other, bw) in [
        (Mode::Silk, Bandwidth::Wide),
        (Mode::Hybrid, Bandwidth::Full),
    ] {
        let decode = |schedule: &dyn Fn(usize) -> Mode| {
            let mut cfg = config(1, 32_000, true, FrameDuration::Ms20);
            cfg.bandwidth = Some(bw);
            let mut enc = Encoder::new(&cfg).expect("valid");
            let mut dec = Decoder::new(1).expect("decoder");
            let mut out = vec![];
            let mut p = vec![];
            for (i, frame) in pcm.as_chunks::<960>().0.iter().enumerate() {
                enc.set_mode_and_bandwidth(Some(schedule(i)), Some(bw));
                enc.encode(frame, &mut p).expect("encodes");
                dec.decode(Some(&p), &mut out).expect("decodes");
                assert_eq!(dec.final_range(), enc.final_range());
            }
            out
        };
        let switched = decode(&|i| {
            if (15..30).contains(&i) {
                other
            } else {
                Mode::Celt
            }
        });
        let celt = decode(&|_| Mode::Celt);
        let steady_other = decode(&|_| other);
        for switch in [15 * 960, 30 * 960] {
            let near = switch..switch + 2 * 960;
            let got = roughness(&switched, 1, near.clone());
            let base = roughness(&celt, 1, near.clone()).max(roughness(&steady_other, 1, near));
            assert!(got < 3.0 * base, "{other:?} at {switch}: {got} vs {base}");
        }
    }
}

#[test]
fn fec_lets_the_decoder_recover_a_lost_packet() {
    let pcm = speech(1, 2.0);
    let mut cfg = config(1, 24_000, true, FrameDuration::Ms20);
    cfg.signal = Signal::Voice;
    cfg.inband_fec = true;
    cfg.packet_loss_percent = 20;
    let (packets, clean) = encode_and_decode(&cfg, &pcm);
    let delay = 312;
    let (mut with_fec, mut concealed) = (0.0f64, 0.0f64);
    for lost in (10..packets.len() - 1).step_by(7) {
        for use_fec in [true, false] {
            let mut dec = Decoder::new(1).expect("decoder");
            let mut out = vec![];
            for (i, p) in packets.iter().enumerate() {
                if i == lost {
                    if use_fec {
                        dec.decode_fec(&packets[i + 1], &mut out).expect("fec");
                    } else {
                        dec.decode(None, &mut out).expect("conceals");
                    }
                } else {
                    dec.decode(Some(p), &mut out).expect("decodes");
                }
            }
            let at = lost * 960;
            let err: f64 = (at..at + 960)
                .map(|k| f64::from(out[k] - clean[k]).powi(2))
                .sum();
            if use_fec {
                with_fec += err;
            } else {
                concealed += err;
            }
        }
    }
    let _ = delay;
    assert!(with_fec * 2.0 < concealed, "{with_fec} vs {concealed}");
}

#[test]
fn dtx_sends_silence_as_toc_bytes() {
    // Digital silence: coded for 200 ms, then a TOC byte for each packet but every 400 ms.
    let mut pcm = speech(1, 0.5);
    pcm.extend(std::iter::repeat_n(0.0, 48_000 * 2));
    let mut cfg = config(1, 16_000, true, FrameDuration::Ms20);
    cfg.dtx = true;
    let (packets, _) = encode_and_decode(&cfg, &pcm);
    let silent = &packets[25 + 12..];
    let toc_only = silent.iter().filter(|p| p.len() == 1).count();
    assert!(
        toc_only * 10 >= silent.len() * 9,
        "{toc_only} of {}",
        silent.len()
    );
    let (plain, _) = encode_and_decode(&config(1, 16_000, true, FrameDuration::Ms20), &pcm);
    assert!(plain[25 + 12..].iter().all(|p| p.len() > 1));
}

#[test]
fn speech_quality_at_low_rates() {
    // The segmental SNR of mono speech at SILK and hybrid rates, measured less 0.5 dB. Through
    // opusdec on 6 s, libopus 1.4's opusenc at complexity 9 with --speech gives the same
    // within 0.1 dB (README).
    let pcm = speech(1, 2.0);
    let mut got = vec![];
    for bitrate in [8_000, 12_000, 16_000, 24_000, 32_000] {
        let mut cfg = config(1, bitrate, true, FrameDuration::Ms20);
        cfg.signal = Signal::Voice;
        let (_, out) = encode_and_decode(&cfg, &pcm);
        got.push((bitrate, seg_snr(&pcm, &out, 1, 312)));
    }
    // Measured: 8.95 (SILK NB), 8.40 (SILK WB), 8.43, 8.68 and 8.85 (hybrid FB) dB.
    for ((bitrate, snr), min) in got.iter().zip([8.4, 7.9, 7.9, 8.1, 8.3]) {
        assert!(*snr > min, "{bitrate}: {snr} dB");
    }
}

#[test]
fn vbr_stays_near_the_bitrate_in_silk_and_hybrid() {
    for (bitrate, signal_hint) in [
        (10_000u32, Signal::Auto),
        (16_000, Signal::Auto),
        (32_000, Signal::Voice),
    ] {
        let mut cfg = config(1, bitrate, true, FrameDuration::Ms20);
        cfg.signal = signal_hint;
        let pcm = speech(1, 4.0);
        let (packets, _) = encode_and_decode(&cfg, &pcm);
        let rate = packets.iter().map(Vec::len).sum::<usize>() as f64 * 8.0 / 4.0;
        assert!(
            (rate / f64::from(bitrate) - 1.0).abs() < 0.25,
            "{bitrate}: {rate}"
        );
    }
}

#[test]
fn the_voip_application_filters_out_rumble() {
    // A 20 Hz rumble under the speech: VoIP's high-pass removes most of it, Audio's 3 Hz DC
    // filter keeps it.
    let tau = 2.0 * std::f32::consts::PI;
    let pcm: Vec<f32> = speech(1, 1.0)
        .iter()
        .enumerate()
        .map(|(i, v)| v + 0.2 * libm::sinf(tau * 20.0 * i as f32 / 48_000.0))
        .collect();
    let level = |app| {
        let mut cfg = config(1, 24_000, true, FrameDuration::Ms20);
        cfg.application = app;
        cfg.mode = Some(Mode::Silk);
        cfg.bandwidth = Some(Bandwidth::Wide);
        let (_, out) = encode_and_decode(&cfg, &pcm);
        // The 20 Hz component's amplitude, by correlation.
        let (mut s, mut c) = (0.0f64, 0.0f64);
        for (i, v) in out.iter().enumerate().skip(9600) {
            let ph = f64::from(tau) * 20.0 * i as f64 / 48_000.0;
            s += f64::from(*v) * libm::sin(ph);
            c += f64::from(*v) * libm::cos(ph);
        }
        2.0 * (s * s + c * c).sqrt() / (out.len() - 9600) as f64
    };
    let audio = level(Application::Audio);
    let voip = level(Application::Voip);
    assert!(voip * 4.0 < audio, "{voip} vs {audio}");
}

#[test]
fn the_bitrate_can_change_between_packets() {
    let mut enc = Encoder::new(&config(1, 64_000, false, FrameDuration::Ms20)).expect("valid");
    let pcm = signal("tone", 1, 0.02);
    let mut p = vec![];
    enc.encode(&pcm, &mut p).expect("encodes");
    assert_eq!(p.len(), 160);
    enc.set_bitrate(32_000).expect("a bitrate");
    enc.encode(&pcm, &mut p).expect("encodes");
    assert_eq!(p.len(), 80);
    assert!(enc.set_bitrate(0).is_err());
}

/// The segmental SNR in dB of `got` against `want` (`channels` interleaved), which `got` lags
/// by `delay` samples: the mean over 20 ms segments louder than -50 dBFS of their SNR, each
/// held to -10 to 35 dB.
pub(crate) fn seg_snr(want: &[f32], got: &[f32], channels: usize, delay: usize) -> f64 {
    let seg = 960 * channels;
    let mut sum = 0.0;
    let mut count = 0;
    let mut at = delay * channels + 4800 * channels;
    while at + seg <= got.len() {
        let (mut sig, mut err) = (0.0f64, 0.0f64);
        for i in at..at + seg {
            let w = f64::from(want[i - delay * channels]);
            sig += w * w;
            err += (w - f64::from(got[i])).powi(2);
        }
        if sig / seg as f64 > 1e-5 {
            sum += (10.0 * libm::log10(sig / err.max(1e-20))).clamp(-10.0, 35.0);
            count += 1;
        }
        at += seg;
    }
    sum / f64::from(count.max(1))
}

fn any_mode() -> impl proptest::strategy::Strategy<Value = Option<Mode>> {
    proptest::sample::select(vec![
        None,
        Some(Mode::Silk),
        Some(Mode::Hybrid),
        Some(Mode::Celt),
    ])
}

fn any_bandwidth() -> impl proptest::strategy::Strategy<Value = Option<Bandwidth>> {
    use Bandwidth::*;
    proptest::sample::select(vec![
        None,
        Some(Narrow),
        Some(Medium),
        Some(Wide),
        Some(SuperWide),
        Some(Full),
    ])
}

fn any_duration() -> impl proptest::strategy::Strategy<Value = FrameDuration> {
    use FrameDuration::*;
    proptest::sample::select(vec![Ms2_5, Ms5, Ms10, Ms20, Ms40, Ms60])
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]

    #[test]
    fn any_config_schedule_and_signal_decodes_with_the_final_range(
        channels in 1u8..=2,
        bitrates in proptest::collection::vec(500u32..300_000, 1..5),
        vbr in proptest::bool::ANY,
        constrained in proptest::bool::ANY,
        d in any_duration(),
        hint in proptest::sample::select(vec![Signal::Auto, Signal::Voice, Signal::Music]),
        voip in proptest::bool::ANY,
        modes in proptest::collection::vec(any_mode(), 1..4),
        bandwidth in any_bandwidth(),
        (fec, loss, dtx) in (proptest::bool::ANY, 0u8..=100, proptest::bool::ANY),
        frames in proptest::collection::vec(
            (
                (10.0f32..20_000.0, 0.0f32..1.0, proptest::bool::ANY, 0.0f32..4.0),
                proptest::collection::vec((0usize..6000, proptest::num::u32::ANY), 0..3),
            ),
            1..6,
        ),
    ) {
        let mut cfg = config(channels, bitrates[0], vbr, d);
        cfg.constrained_vbr = constrained;
        cfg.signal = hint;
        if voip {
            cfg.application = Application::Voip;
        }
        cfg.bandwidth = bandwidth;
        cfg.inband_fec = fec;
        cfg.packet_loss_percent = loss;
        cfg.dtx = dtx;
        let ch = usize::from(channels);
        let n = d.samples();
        let mut seed = bitrates[0];
        let mut enc = Encoder::new(&cfg).expect("valid");
        let mut dec = Decoder::new(ch).expect("decoder");
        let mut out = vec![];
        for (i, (r, raw)) in frames.iter().enumerate() {
            let pcm = recipe_frame(n, ch, *r, raw, &mut seed);
            enc.set_bitrate(bitrates[i % bitrates.len()]).expect("a bitrate");
            enc.set_mode_and_bandwidth(modes[i % modes.len()], bandwidth);
            let mut p = vec![];
            enc.encode(&pcm, &mut p).expect("encodes");
            let parsed = Packet::parse(&p).expect("parses");
            proptest::prop_assert_eq!(parsed.duration(), n);
            if !vbr && !dtx {
                // CBR: the size the bitrate gives, at most a frame's 1276 bytes.
                let rate12 = 12 * 48_000 / n as u32;
                let b = bitrates[i % bitrates.len()].clamp(500, 300_000 * u32::from(channels));
                let want = ((12 * b / 8 + rate12 / 2) / rate12).clamp(1, 1276) as usize;
                proptest::prop_assert_eq!(p.len(), want.max(1 + usize::from(n > 960)));
            }
            dec.decode(Some(&p), &mut out).expect("decodes");
            proptest::prop_assert_eq!(dec.final_range(), enc.final_range(), "packet {}", i);
        }
        proptest::prop_assert!(out.iter().all(|v| v.is_finite()));
    }
}

#[test]
fn silk_and_hybrid_give_the_same_bytes_on_every_platform() {
    // Frozen: a change here changes the encoder's output, which must be on purpose (last:
    // the tonality analysis). SILK is integer arithmetic, CELT float through libm only.
    let mut cases = vec![];
    let mut add = |channels: u8, bitrate, vbr, d, f: &dyn Fn(&mut EncoderConfig)| {
        let mut cfg = config(channels, bitrate, vbr, d);
        f(&mut cfg);
        let pcm = speech(usize::from(channels), 0.6);
        cases.push(digest(&cfg, &pcm));
    };
    add(1, 12_000, true, FrameDuration::Ms20, &|c| {
        c.signal = Signal::Voice;
    });
    add(1, 24_000, true, FrameDuration::Ms20, &|c| {
        c.signal = Signal::Voice;
    });
    add(2, 32_000, false, FrameDuration::Ms10, &|c| {
        c.signal = Signal::Voice;
    });
    add(1, 16_000, true, FrameDuration::Ms60, &|c| {
        c.mode = Some(Mode::Silk);
    });
    add(2, 48_000, true, FrameDuration::Ms40, &|c| {
        c.mode = Some(Mode::Hybrid);
    });
    add(1, 64_000, false, FrameDuration::Ms40, &|c| {
        c.mode = Some(Mode::Celt);
    });
    add(1, 20_000, true, FrameDuration::Ms20, &|c| {
        c.signal = Signal::Voice;
        c.inband_fec = true;
        c.packet_loss_percent = 10;
        c.application = Application::Voip;
    });
    assert_eq!(
        cases,
        [
            (693, 11736134073601484034),
            (1556, 7782954769236046977),
            (2400, 16023036227370905054),
            (1043, 14814359666415212676),
            (2709, 7930724760705322200),
            (4800, 10451175142009782759),
            (1332, 15683301958387451202)
        ]
    );
}
