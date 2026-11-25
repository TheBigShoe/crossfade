use super::*;
use crate::Decoder;

/// One encoder configuration for the tests.
#[derive(Debug, Clone, Copy)]
pub(super) struct Cfg {
    pub(super) channels: usize,
    pub(super) khz: i32,
    pub(super) ms: i32,
    pub(super) bitrate: i32,
    pub(super) complexity: i32,
    pub(super) cbr: bool,
}

impl Cfg {
    fn control(&self) -> EncControl {
        EncControl {
            n_channels_api: self.channels,
            n_channels_internal: self.channels,
            max_internal_sample_rate: self.khz * 1000,
            min_internal_sample_rate: self.khz * 1000,
            desired_internal_sample_rate: self.khz * 1000,
            payload_size_ms: self.ms,
            bit_rate: self.bitrate,
            complexity: self.complexity,
            use_cbr: self.cbr,
            ..EncControl::default()
        }
    }

    /// The TOC byte of a SILK-only packet with one frame.
    fn toc(&self, internal_rate: i32) -> u8 {
        let bw = match internal_rate {
            8000 => 0,
            12000 => 1,
            _ => 2,
        };
        let size = match self.ms {
            10 => 0,
            20 => 1,
            40 => 2,
            _ => 3,
        };
        ((bw * 4 + size) << 3) | (u8::from(self.channels == 2) << 2)
    }
}

/// A packet and the encoder's final range.
pub(super) struct Coded {
    pub(super) packet: Vec<u8>,
    pub(super) rng: u32,
    /// The frame overran its budget (as libopus's can, near the smallest budgets it gives
    /// SILK) and is sent as a TOC byte alone, which a decoder conceals.
    pub(super) overrun: bool,
}

/// The packet's byte budget, as libopus's Opus layer sets it: the bitrate's share in CBR,
/// the largest packet otherwise. libopus never asks SILK for less than about 6 kb/s per
/// channel (more for 10 ms frames, and its bandwidth choice wants more for MB and WB): at
/// smaller budgets its own encoder can overrun the packet. The budget stays above that.
pub(super) fn max_bytes(cfg: Cfg) -> usize {
    if cfg.cbr {
        let per_channel = match cfg.khz {
            8 => 6000,
            12 => 8000,
            _ => 10000,
        };
        let floor = per_channel * if cfg.ms == 10 { 3 } else { 2 } / 2 * cfg.channels as i32;
        let floor_bytes = (floor * cfg.ms + 7999) / 8000;
        (cfg.bitrate * cfg.ms / 8000).max(floor_bytes).min(1275) as usize
    } else {
        1275
    }
}

/// Encodes `pcm` (48 kHz, interleaved) packet by packet as SILK-only Opus packets (CBR ones
/// not padded to their budget).
pub(super) fn encode(cfg: Cfg, pcm: &[i16]) -> Vec<Coded> {
    let mut e = SilkEncoder::new();
    let mut c = cfg.control();
    let n = 48 * cfg.ms as usize * cfg.channels;
    let max_bytes = max_bytes(cfg);
    pcm.chunks_exact(n)
        .map(|frame| {
            let mut enc = RangeEncoder::new(max_bytes - 1);
            c.max_bits = (max_bytes as i32 - 1) * 8;
            let bytes = e
                .encode(&mut c, frame, Some(&mut enc), Prefill::No, Some(true))
                .expect("encodes");
            let nb = ((enc.tell() + 7) >> 3) as usize;
            let rng = enc.rng();
            let mut packet = vec![cfg.toc(c.internal_sample_rate)];
            let overrun = nb > max_bytes - 1;
            if bytes > 0 && !overrun {
                let body = enc.done().expect("fits");
                packet.extend_from_slice(&body[..nb]);
            }
            Coded {
                packet,
                rng,
                overrun,
            }
        })
        .collect()
}

/// Decodes every packet, checking each final range; returns the 48 kHz output.
pub(super) fn decode_checked(cfg: Cfg, coded: &[Coded]) -> Vec<f32> {
    let mut d = Decoder::new(cfg.channels).expect("decoder");
    let mut all = Vec::new();
    for (i, c) in coded.iter().enumerate() {
        let mut out = Vec::new();
        let n = d
            .decode(Some(&c.packet), &mut out)
            .unwrap_or_else(|e| panic!("{cfg:?} packet {i} ({} bytes): {e:?}", c.packet.len()));
        assert_eq!(n, 48 * cfg.ms as usize, "{cfg:?} packet {i}");
        if c.packet.len() > 1 {
            assert_eq!(d.final_range(), c.rng, "{cfg:?} packet {i}: final range");
        }
        all.extend_from_slice(&out);
    }
    all
}

/// Deterministic test signals at 48 kHz.
pub(super) mod signals {
    /// xorshift32 noise in `-amp..amp`.
    pub(in super::super) fn noise(len: usize, seed: u32, amp: i32) -> Vec<i16> {
        let mut s = seed.wrapping_mul(2_654_435_761) | 1;
        (0..len)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (i32::from((s >> 16) as u16 as i16) * amp / 32768) as i16
            })
            .collect()
    }

    /// A glottal pulse train at `f0` Hz through three formant resonators (500, 1500,
    /// 2500 Hz), the pitch gliding a little: voiced-speech-like.
    pub(in super::super) fn voiced(len: usize, f0: f64, amp: f64) -> Vec<i16> {
        let fs = 48_000.0;
        let formants = [(500.0, 80.0), (1500.0, 120.0), (2500.0, 160.0)];
        let coefs: Vec<(f64, f64)> = formants
            .iter()
            .map(|&(f, bw)| {
                let r = libm::exp(-core::f64::consts::PI * bw / fs);
                (
                    2.0 * r * libm::cos(2.0 * core::f64::consts::PI * f / fs),
                    -r * r,
                )
            })
            .collect();
        let mut state = [(0.0f64, 0.0f64); 3];
        let mut phase = 0.0;
        let mut out = Vec::with_capacity(len);
        let mut peak = 1e-9f64;
        let mut raw = Vec::with_capacity(len);
        for n in 0..len {
            let f = f0 * (1.0 + 0.1 * libm::sin(2.0 * core::f64::consts::PI * n as f64 / fs * 3.0));
            phase += f / fs;
            let mut x = if phase >= 1.0 {
                phase -= 1.0;
                1.0
            } else {
                0.0
            };
            for (k, &(a1, a2)) in coefs.iter().enumerate() {
                let y = x + a1 * state[k].0 + a2 * state[k].1;
                state[k].1 = state[k].0;
                state[k].0 = y;
                x = y;
            }
            peak = peak.max(x.abs());
            raw.push(x);
        }
        for x in raw {
            out.push((x / peak * amp) as i16);
        }
        out
    }

    pub(in super::super) fn tone(len: usize, f: f64, amp: f64) -> Vec<i16> {
        (0..len)
            .map(|n| {
                (amp * libm::sin(2.0 * core::f64::consts::PI * f * n as f64 / 48_000.0)) as i16
            })
            .collect()
    }

    pub(in super::super) fn clicks(len: usize) -> Vec<i16> {
        (0..len)
            .map(|n| if n % 4801 == 0 { 30000 } else { 0 })
            .collect()
    }

    /// Two channels interleaved.
    pub(in super::super) fn stereo(l: &[i16], r: &[i16]) -> Vec<i16> {
        l.iter().zip(r).flat_map(|(&a, &b)| [a, b]).collect()
    }
}

use signals::*;

fn all_signals(len: usize) -> Vec<(&'static str, Vec<i16>)> {
    vec![
        ("silence", vec![0; len]),
        ("voiced", voiced(len, 140.0, 12000.0)),
        ("noise", noise(len, 5, 8000)),
        ("tone", tone(len, 440.0, 10000.0)),
        ("clicks", clicks(len)),
        ("dc", vec![9000; len]),
        (
            "full scale",
            noise(len, 9, 32767)
                .iter()
                .map(|&x| if x < 0 { i16::MIN } else { i16::MAX })
                .collect(),
        ),
    ]
}

/// Every bandwidth and packet size, a bitrate from about 6 to 40 kb/s, the complexities.
const CONFIGS: [(i32, i32, i32, i32); 12] = [
    (8, 10, 6000, 0),
    (8, 20, 8000, 9),
    (8, 40, 10000, 2),
    (8, 60, 12000, 5),
    (12, 10, 14000, 9),
    (12, 20, 12000, 1),
    (12, 40, 18000, 7),
    (12, 60, 9000, 3),
    (16, 10, 30000, 4),
    (16, 20, 24000, 9),
    (16, 40, 16000, 10),
    (16, 60, 40000, 6),
];

#[test]
fn mono_packets_decode_with_the_encoders_final_range() {
    let len = 48_000 / 2;
    for (name, x) in all_signals(len) {
        for (k, (khz, ms, bitrate, complexity)) in CONFIGS.into_iter().enumerate() {
            let cbr = k % 3 == 1;
            let cfg = Cfg {
                channels: 1,
                khz,
                ms,
                bitrate,
                complexity,
                cbr,
            };
            let coded = encode(cfg, &x);
            assert!(!coded.is_empty(), "{name}");
            decode_checked(cfg, &coded);
        }
    }
}

#[test]
fn stereo_packets_decode_with_the_encoders_final_range() {
    let len = 48_000 / 2;
    let a = voiced(len, 140.0, 12000.0);
    let b = voiced(len, 190.0, 9000.0);
    let n = noise(len, 77, 6000);
    let silent = vec![0i16; len];
    let half: Vec<i16> = a.iter().map(|&x| x / 2).collect();
    let pairs: [(&str, &[i16], &[i16]); 5] = [
        ("two voices", &a, &b),
        ("identical", &a, &a),
        ("panned", &a, &half),
        ("noise and silence", &n, &silent),
        ("silence", &silent, &silent),
    ];
    for (name, l, r) in pairs {
        let x = stereo(l, r);
        for (k, (khz, ms, bitrate, complexity)) in CONFIGS.into_iter().enumerate() {
            let cbr = k % 4 == 1;
            let cfg = Cfg {
                channels: 2,
                khz,
                ms,
                bitrate: bitrate * 2,
                complexity,
                cbr,
            };
            let coded = encode(cfg, &x);
            assert!(!coded.is_empty(), "{name}");
            decode_checked(cfg, &coded);
        }
    }
}

/// The quantiser runs the decoder's synthesis: after every packet the encoder's
/// reconstruction (the last 20 ms of `xq`) is the decoder's output at the internal rate
/// (`out_buf`, before resampling), sample for sample, in each coded channel.
#[test]
fn the_encoders_reconstruction_is_the_decoders_output() {
    use crate::range::RangeDecoder;
    use crate::silk::{SilkDecoder, SilkFrame};
    let len = 48_000 / 2;
    let a = voiced(len, 140.0, 12000.0);
    let b = noise(len, 3, 5000);
    for (channels, x) in [(1usize, a.clone()), (2, stereo(&a, &b))] {
        for (khz, ms, bitrate, complexity) in CONFIGS {
            let cfg = Cfg {
                channels,
                khz,
                ms,
                bitrate: bitrate * channels as i32,
                complexity,
                cbr: false,
            };
            let mut e = SilkEncoder::new();
            let mut c = cfg.control();
            let mut d = SilkDecoder::new(channels);
            let n = 48 * ms as usize * channels;
            for (i, frame) in x.chunks_exact(n).enumerate() {
                let mut enc = RangeEncoder::new(1274);
                c.max_bits = 1274 * 8;
                e.encode(&mut c, frame, Some(&mut enc), Prefill::No, Some(true))
                    .expect("encodes");
                let nb = ((enc.tell() + 7) >> 3) as usize;
                let body = enc.done().expect("fits");
                let mut dec = RangeDecoder::new(&body[..nb]);
                let f = SilkFrame {
                    frame_size: 48 * ms as usize,
                    internal_rate: (khz * 1000) as u32,
                    stream_channels: channels,
                    fec: false,
                };
                let mut out = vec![0.0f32; f.frame_size * channels];
                d.decode(Some(&mut dec), &f, &mut out).expect("decodes");
                for ch in 0..channels {
                    if ch == 1 && e.mid_only(0) {
                        continue;
                    }
                    let enc_st = &e.channels[ch];
                    let dec_st = &d.state.channels[ch];
                    let l = enc_st.ltp_mem_length;
                    assert_eq!(
                        enc_st.s_nsq.xq[..l],
                        dec_st.out_buf[..l],
                        "{cfg:?} packet {i} channel {ch}"
                    );
                }
            }
        }
    }
}

/// Random 16-bit input of a few shapes: white, bursts, a square wave, extremes.
fn random_input(seed: u32, shape: u8, amp: i32, len: usize) -> Vec<i16> {
    let n = noise(len, seed, amp.max(1));
    match shape % 4 {
        0 => n,
        1 => n
            .iter()
            .enumerate()
            .map(|(i, &x)| if (i / 480) % 3 == 0 { x } else { 0 })
            .collect(),
        2 => (0..len)
            .map(|i| {
                if (i / (37 + seed as usize % 200)) % 2 == 0 {
                    amp as i16
                } else {
                    -(amp as i16)
                }
            })
            .collect(),
        _ => n
            .iter()
            .map(|&x| if x >= 0 { i16::MAX } else { i16::MIN })
            .collect(),
    }
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig { cases: 256, ..Default::default() })]

    /// Any configuration, any input: no panic, and every packet decodes with the encoder's
    /// final range.
    #[test]
    fn any_input_and_config_make_decodable_packets(
        channels in 1usize..=2,
        khz in proptest::sample::select(vec![8, 12, 16]),
        ms in proptest::sample::select(vec![10, 20, 40, 60]),
        bitrate in 5_000i32..80_000,
        complexity in 0i32..=10,
        cbr: bool,
        seed: u32,
        shape: u8,
        amp in 0i32..=32767,
    ) {
        let cfg = Cfg { channels, khz, ms, bitrate, complexity, cbr };
        let len = 3 * 48 * ms as usize * channels;
        let x = random_input(seed, shape, amp, len);
        let coded = encode(cfg, &x);
        // Only a CBR budget can be overrun.
        proptest::prop_assert!(cbr || coded.iter().all(|c| !c.overrun));
        decode_checked(cfg, &coded);
    }
}

/// The delay (in samples at 48 kHz) that best lines `out` up with `x`, and the SNR and
/// segmental SNR (20 ms segments, each clamped to -10..35 dB) after it, in dB, skipping the
/// first 100 ms.
pub(super) fn quality(x: &[i16], out: &[f32]) -> (usize, f64, f64) {
    let y: Vec<f64> = out.iter().map(|&v| f64::from(v) * 32768.0).collect();
    let skip = 4800;
    let n = x.len().min(y.len()) - 1000;
    let mut best = (0, f64::MIN);
    for d in 0..600 {
        let c: f64 = (skip..n).map(|i| f64::from(x[i]) * y[i + d]).sum();
        if c > best.1 {
            best = (d, c);
        }
    }
    let d = best.0;
    let (mut sig, mut err) = (0.0, 0.0);
    let mut seg = Vec::new();
    for s in (skip..n - 960).step_by(960) {
        let (mut ss, mut ee) = (0.0, 0.0);
        for i in s..s + 960 {
            let e = f64::from(x[i]) - y[i + d];
            ss += f64::from(x[i]) * f64::from(x[i]);
            ee += e * e;
        }
        sig += ss;
        err += ee;
        if ss > 1e3 {
            seg.push((10.0 * libm::log10(ss / ee.max(1e-9))).clamp(-10.0, 35.0));
        }
    }
    let snr = 10.0 * libm::log10(sig / err.max(1e-9));
    let segsnr = seg.iter().sum::<f64>() / seg.len().max(1) as f64;
    (d, snr, segsnr)
}

#[test]
#[ignore]
fn measure_quality_and_rate() {
    let len = 48_000 * 4;
    let x = voiced(len, 140.0, 12000.0);
    for (khz, ms, bitrate) in [
        (8, 20, 8000),
        (8, 20, 12000),
        (12, 20, 12000),
        (12, 20, 16000),
        (16, 20, 12000),
        (16, 20, 16000),
        (16, 20, 24000),
        (16, 20, 32000),
        (16, 20, 40000),
        (16, 60, 24000),
        (16, 10, 24000),
    ] {
        for cbr in [false, true] {
            let cfg = Cfg {
                channels: 1,
                khz,
                ms,
                bitrate,
                complexity: 10,
                cbr,
            };
            let coded = encode(cfg, &x);
            let out = decode_checked(cfg, &coded);
            let (d, snr, seg) = quality(&x, &out);
            let bytes: usize = coded.iter().map(|c| c.packet.len()).sum();
            let kbps = bytes as f64 * 8.0 / (len as f64 / 48_000.0) / 1000.0;
            let (mn, mx) = coded
                .iter()
                .map(|c| c.packet.len())
                .fold((usize::MAX, 0), |(a, b), l| (a.min(l), b.max(l)));
            eprintln!(
                "{khz} kHz {ms} ms {bitrate} cbr {cbr}: delay {d} snr {snr:.1} seg {seg:.1} rate {kbps:.2} kb/s packets {mn}..{mx}"
            );
        }
    }
}

/// Voiced-speech-like input decodes close to the input, the more so the higher the rate:
/// segmental SNR floors, measured (complexity 10, VBR) with about 2.5 dB of margin.
#[test]
fn voiced_input_decodes_above_quality_floors() {
    let len = 48_000 * 2;
    let x = voiced(len, 140.0, 12000.0);
    let mut last = f64::MIN;
    for (khz, bitrate, floor) in [
        (8, 8000, 9.0),
        (12, 16000, 12.5),
        (16, 24000, 15.0),
        (16, 40000, 19.5),
    ] {
        let cfg = Cfg {
            channels: 1,
            khz,
            ms: 20,
            bitrate,
            complexity: 10,
            cbr: false,
        };
        let out = decode_checked(cfg, &encode(cfg, &x));
        let (delay, _, seg) = quality(&x, &out);
        assert!((300..=320).contains(&delay), "{cfg:?}: delay {delay}");
        assert!(seg > floor, "{cfg:?}: segmental SNR {seg:.1} dB");
        if khz == 16 {
            assert!(seg > last, "{cfg:?}: {seg:.1} dB, not above {last:.1}");
            last = seg;
        }
    }
}

/// CBR fills each 20 ms packet's budget to within 15 % without exceeding it; VBR spends
/// less on a predictable signal and more on noise, around the target.
#[test]
fn packet_sizes_track_the_bitrate() {
    let len = 48_000 * 2;
    let voice = voiced(len, 140.0, 12000.0);
    let hiss = noise(len, 21, 6000);
    for (khz, bitrate) in [(8, 8000), (12, 12000), (16, 16000), (16, 32000)] {
        let cfg = Cfg {
            channels: 1,
            khz,
            ms: 20,
            bitrate,
            complexity: 9,
            cbr: true,
        };
        let budget = max_bytes(cfg);
        for x in [&voice, &hiss] {
            let coded = encode(cfg, x);
            decode_checked(cfg, &coded);
            let sizes: Vec<usize> = coded.iter().skip(5).map(|c| c.packet.len()).collect();
            let mean = sizes.iter().sum::<usize>() as f64 / sizes.len() as f64;
            assert!(
                sizes.iter().all(|&s| s <= budget),
                "{cfg:?}: {sizes:?} over {budget}"
            );
            assert!(
                mean > 0.85 * budget as f64,
                "{cfg:?}: mean {mean:.1} of {budget}"
            );
        }
    }
    for (khz, bitrate) in [(8, 10000), (16, 24000)] {
        let cfg = Cfg {
            channels: 1,
            khz,
            ms: 20,
            bitrate,
            complexity: 9,
            cbr: false,
        };
        let rate = |x: &[i16]| {
            let coded = encode(cfg, x);
            decode_checked(cfg, &coded);
            let bytes: usize = coded.iter().map(|c| c.packet.len()).sum();
            bytes as f64 * 8.0 / (x.len() as f64 / 48_000.0)
        };
        let (rv, rn) = (rate(&voice), rate(&hiss));
        let target = f64::from(bitrate);
        assert!(
            rv > 0.35 * target && rv < target,
            "{cfg:?}: voice at {rv:.0} b/s"
        );
        assert!(
            rn > 0.75 * target && rn < 1.4 * target,
            "{cfg:?}: noise at {rn:.0} b/s"
        );
    }
}
