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
    for threads in [1, 3] {
        let mut whole = session(relative, threads);
        let channels = usize::from(whole.stream().audio.layout().channels());
        let all = rest(&mut whole);
        let mut s = session(relative, threads);
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
