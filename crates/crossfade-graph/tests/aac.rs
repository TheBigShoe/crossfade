//! AAC through the engine (built with `--features aac`): MP4 and ADTS through `DecodeSession`,
//! gapless, in WAVE channel order, and seek-then-decode equal to decode-then-slice.
#![cfg(feature = "aac")]

#[path = "../../crossfade-format/tests/support/mp4_signals.rs"]
mod mp4_signals;

use crossfade_core::{AudioFrame, ChannelLayout, Limits, Samples};
use crossfade_graph::DecodeSession;
use crossfade_testkit::memory::MemoryIo;

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../crossfade-format/tests/fixtures"
);

fn session(relative: &str, threads: usize) -> DecodeSession {
    let mut io = MemoryIo::new();
    io.insert(
        "in",
        std::fs::read(format!("{FIXTURES}/{relative}")).unwrap(),
    );
    let limits = Limits {
        max_threads: threads,
        ..Limits::default()
    };
    DecodeSession::open("in", &mut io, &limits).unwrap()
}

/// Everything from the session's position to the end, interleaved.
fn rest(s: &mut DecodeSession) -> Vec<f32> {
    let mut f = AudioFrame::new(&s.stream().audio);
    let mut out = Vec::new();
    let mut at = s.position();
    while s.next_frame(&mut f).unwrap() {
        assert_eq!(f.pts as u64, at);
        at += f.frames() as u64;
        let Samples::F32(v) = &f.samples else {
            panic!("{:?}", f.samples.format())
        };
        out.extend_from_slice(v);
    }
    out
}

/// Signal-to-error ratio in dB of `got` (-1..1) against integer `want` of `bits`, per channel.
fn snr(got: &[f32], want: &[i32], bits: u8, channels: usize, lag: isize) -> Vec<f64> {
    let scale = f64::from(1u32 << (bits - 1));
    let frames = want.len() / channels;
    (0..channels)
        .map(|c| {
            let (mut sig, mut err) = (0.0, 0.0);
            for i in 64..frames - 64 {
                let j = (i as isize + lag) as usize;
                let w = f64::from(want[i * channels + c]) / scale;
                let g = f64::from(got[j * channels + c]);
                sig += w * w;
                err += (g - w) * (g - w);
            }
            10.0 * (sig / err).log10()
        })
        .collect()
}

#[test]
fn an_aac_m4a_decodes_gapless_to_its_source() {
    let source = mp4_signals::signal("stereo16");
    let mut s = session("mp4/aac-stereo16.m4a", 1);
    assert_eq!(s.stream().frames, Some(source.frames() as u64));
    let got = rest(&mut s);
    // Priming dropped and padding cut: exactly the source's length.
    assert_eq!(got.len(), source.samples.len());
    // And lined up with it: the best match is at no lag, and a close one.
    let at = |lag| snr(&got, &source.samples, 16, 2, lag);
    for c in 0..2 {
        assert!(at(0)[c] > 30.0, "{:?}", at(0));
        assert!(at(0)[c] > at(1)[c] + 10.0 && at(0)[c] > at(-1)[c] + 10.0);
    }
}

#[test]
fn aac_channels_come_in_the_wave_order_the_mp4_reader_names() {
    for channels in 3..=8 {
        let name = format!("ch{channels}");
        let source = mp4_signals::signal(&name);
        let mut s = session(&format!("mp4/aac-{name}.m4a"), 1);
        let mask = mp4_signals::mask(channels);
        assert_eq!(
            s.stream().audio.layout(),
            &ChannelLayout::from_mask(mask, channels as u16).unwrap(),
            "{name}"
        );
        let got = rest(&mut s);
        assert_eq!(got.len(), source.samples.len(), "{name}");
        // Each channel carries its own tone: a channel out of place would match badly. The
        // encoder low-passes the LFE (WAVE position 3 when the mask has it), whose tones lie
        // above its band, so it is nearly silent instead.
        let ratios = snr(&got, &source.samples, source.bits, channels, 0);
        let lfe = (mask & 0x8 != 0).then_some(3);
        for (c, r) in ratios.iter().enumerate() {
            if Some(c) == lfe {
                let peak = got[c..]
                    .iter()
                    .step_by(channels)
                    .fold(0f32, |a, b| a.max(b.abs()));
                assert!(peak < 0.05, "{name}: LFE peak {peak}");
            } else {
                assert!(*r > 20.0, "{name}: {ratios:?}");
            }
        }
    }
}

fn seek_equals_slice(relative: &str, targets: &[u64]) {
    let bytes = std::fs::read(format!("{FIXTURES}/{relative}")).unwrap();
    seek_equals_slice_of(relative, &bytes, targets);
}

fn seek_equals_slice_of(relative: &str, bytes: &[u8], targets: &[u64]) {
    let open = |threads| {
        let mut io = MemoryIo::new();
        io.insert("in", bytes.to_vec());
        let limits = Limits {
            max_threads: threads,
            ..Limits::default()
        };
        DecodeSession::open("in", &mut io, &limits).unwrap()
    };
    for threads in [1, 3] {
        let mut whole = open(threads);
        let channels = usize::from(whole.stream().audio.layout().channels());
        let all = rest(&mut whole);
        let mut s = open(threads);
        for &t in targets {
            s.seek(t).unwrap();
            assert_eq!(s.position(), t.min((all.len() / channels) as u64));
            let got = rest(&mut s);
            let from = (t as usize * channels).min(all.len());
            assert!(got == all[from..], "{relative}: seek to {t}");
        }
    }
}

#[test]
fn seeking_an_adts_stream_equals_slicing_its_decode() {
    let targets = [
        0, 1, 1023, 1024, 1025, 5000, 20_480, 33_333, 47_103, 47_104, 90_000, 7,
    ];
    seek_equals_slice("adts/clicks_44k_stereo.aac", &targets);
    seek_equals_slice("adts/tones_48k_51.aac", &[0, 3000, 26_000, 2048, 1]);
}

#[test]
fn seeking_an_aac_m4a_equals_slicing_its_decode() {
    let targets = [
        0, 1, 500, 1024, 2111, 2112, 4000, 9000, 10_006, 10_007, 20_000, 3,
    ];
    seek_equals_slice("mp4/aac-stereo16.m4a", &targets);
    seek_equals_slice("mp4/aac-ch6.m4a", &[0, 1000, 1500, 2000, 17]);
}

/// MSB-first bit writer.
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    used: u32,
}

impl Bits {
    fn put(&mut self, value: u32, n: u32) {
        for i in (0..n).rev() {
            if self.used.is_multiple_of(8) {
                self.bytes.push(0);
            }
            let bit = (value >> i) as u8 & 1;
            *self.bytes.last_mut().unwrap() |= bit << (7 - self.used % 8);
            self.used += 1;
        }
    }
}

/// An ADTS stream (AAC LC, 44.1 kHz, mono) of `frames` blocks whose upper 20 bands are
/// perceptual noise substitution, a different energy per frame.
fn pns_adts(frames: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for f in 0..frames {
        let mut b = Bits::default();
        b.put(0, 3); // ID_SCE
        b.put(0, 4); // tag
        b.put(100, 8); // global_gain
        b.put(0, 4); // reserved bit, ONLY_LONG, sine
        b.put(40, 6); // max_sfb
        b.put(0, 1); // no prediction
        b.put(0, 4); // section: zero book, 20 bands
        b.put(20, 5);
        b.put(13, 4); // section: noise book, 20 bands
        b.put(20, 5);
        b.put(290 + f % 23, 9); // first noise energy
        for band in 1..20 {
            // Scalefactor deltas 0, +1, -1.
            match band % 3 {
                0 => b.put(0x0, 1),
                1 => b.put(0xa, 4),
                _ => b.put(0x4, 3),
            }
        }
        b.put(0, 3); // no pulses, TNS or gain control
        b.put(7, 3); // ID_END
        let len = 7 + b.bytes.len() as u32;
        let mut h = Bits::default();
        h.put(0xFFF, 12);
        h.put(0, 1); // MPEG-4
        h.put(0, 2); // layer
        h.put(1, 1); // no CRC
        h.put(1, 2); // LC
        h.put(4, 4); // 44.1 kHz
        h.put(0, 1);
        h.put(1, 3); // mono
        h.put(0, 4);
        h.put(len, 13);
        h.put(0x7FF, 11);
        h.put(0, 2); // one raw data block
        out.extend_from_slice(&h.bytes);
        out.extend_from_slice(&b.bytes);
    }
    out
}

#[test]
fn seeking_a_stream_with_noise_substitution_equals_slicing_its_decode() {
    // The noise is seeded from each block's bytes, so it is the same after a seek.
    let stream = pns_adts(60);
    let targets = [0, 1, 1024, 5000, 20_480, 33_333, 61_439, 7, 40_000];
    seek_equals_slice_of("pns", &stream, &targets);
    // And it is noise.
    let mut io = MemoryIo::new();
    io.insert("in", stream);
    let mut s = DecodeSession::open("in", &mut io, &Limits::default()).unwrap();
    let all = rest(&mut s);
    let rms = (all.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / all.len() as f64).sqrt();
    assert!(rms > 1e-5 && rms < 0.5, "{rms}");
}
