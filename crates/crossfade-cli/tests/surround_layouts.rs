//! Surround files that differ from what Opus or ALAC store only in the name of the single
//! surround pair (side or back), or carry no layout at all, convert unchanged channel for
//! channel, with a note; anything else is still refused. Through the real binary.

use std::path::PathBuf;
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_crossfade"))
        .args(args)
        .output()
        .unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Success: stderr (the notes), with stdout kept for the caller that wants it.
fn ok(args: &[&str]) -> (String, String) {
    let out = run(args);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{args:?}: {}",
        text(&out.stderr)
    );
    (text(&out.stdout), text(&out.stderr))
}

fn refused(args: &[&str]) -> String {
    let out = run(args);
    assert_ne!(out.status.code(), Some(0), "{args:?} should fail");
    text(&out.stderr)
}

struct Dir(PathBuf);

impl Dir {
    fn new(test: &str) -> Self {
        let p =
            std::env::temp_dir().join(format!("crossfade-surround-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A 16-bit WAV; a channel mask makes it WAVE_FORMAT_EXTENSIBLE, none leaves the channels
/// unlabelled.
fn wav(rate: u32, channels: usize, mask: Option<u32>, samples: &[i16]) -> Vec<u8> {
    let data: Vec<u8> = samples.iter().flat_map(|x| x.to_le_bytes()).collect();
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
    b.extend_from_slice(&(if mask.is_some() { 40u32 } else { 16 }).to_le_bytes());
    b.extend_from_slice(&(if mask.is_some() { 0xFFFEu16 } else { 1 }).to_le_bytes());
    b.extend_from_slice(&(channels as u16).to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    b.extend_from_slice(&(channels as u16 * 2).to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    if let Some(mask) = mask {
        b.extend_from_slice(&22u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(&mask.to_le_bytes());
        b.extend_from_slice(&[
            1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71,
        ]);
    }
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    let riff = (b.len() - 8) as u32;
    b[4..8].copy_from_slice(&riff.to_le_bytes());
    b
}

/// The tone of channel `c`: a different one on each, none a harmonic of another; the LFE
/// (`lfe`) a low one, as it is coded low-passed.
fn tone_hz(c: usize, lfe: Option<usize>) -> f64 {
    if Some(c) == lfe {
        90.0
    } else {
        700.0 + 530.0 * c as f64
    }
}

const FRAMES: usize = 24_000;

fn tones(channels: usize, lfe: Option<usize>) -> Vec<i16> {
    (0..FRAMES)
        .flat_map(|i| {
            (0..channels).map(move |c| {
                let t = i as f64 / 48_000.0;
                (6000.0 * (2.0 * std::f64::consts::PI * tone_hz(c, lfe) * t).sin()) as i16
            })
        })
        .collect()
}

/// The `data` chunk of a WAV file written by the tool.
fn data_of(path: &str) -> Vec<u8> {
    let bytes = std::fs::read(path).unwrap();
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if &bytes[pos..pos + 4] == b"data" {
            return bytes[pos + 8..pos + 8 + len].to_vec();
        }
        pos += 8 + len + (len & 1);
    }
    panic!("no data chunk");
}

fn samples_of(path: &str) -> Vec<i16> {
    data_of(path)
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes(*c))
        .collect()
}

/// How much of channel `c` (of `ch`) lies at `hz`, by the Goertzel recurrence.
fn tone_at(x: &[i16], ch: usize, c: usize, hz: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * hz / 48_000.0;
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for f in x.chunks_exact(ch) {
        let s = f64::from(f[c]) / 32768.0 + 2.0 * w.cos() * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    (s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2)
        .max(0.0)
        .sqrt()
        / (x.len() / ch) as f64
}

/// Every channel of `got` carries its own tone and none of the others.
fn assert_tones_in_place(got: &[i16], channels: usize, lfe: Option<usize>, what: &str) {
    assert_eq!(got.len(), FRAMES * channels, "{what}: length");
    for c in 0..channels {
        let own = tone_at(got, channels, c, tone_hz(c, lfe));
        assert!(own > 0.03, "{what}: channel {c} lost its tone ({own})");
        for d in (0..channels).filter(|&d| d != c) {
            let other = tone_at(got, channels, c, tone_hz(d, lfe));
            assert!(
                other * 10.0 < own,
                "{what}: channel {c} carries channel {d}'s tone ({other} vs {own})"
            );
        }
    }
}

/// A source layout written to `.opus` and read back as 16-bit WAV: the probe's layout name,
/// the note on stderr and the decoded samples.
fn through_opus(
    dir: &Dir,
    channels: usize,
    mask: Option<u32>,
    lfe: Option<usize>,
) -> (String, String, Vec<i16>) {
    let (wav_in, opus, back) = (dir.path("in.wav"), dir.path("o.opus"), dir.path("back.wav"));
    std::fs::write(&wav_in, wav(48_000, channels, mask, &tones(channels, lfe))).unwrap();
    let (_, notes) = ok(&["convert", "--force", &wav_in, "-o", &opus]);
    let (probe, _) = ok(&["probe", &opus]);
    ok(&["convert", "--force", &opus, "-o", &back, "--dither", "none"]);
    (probe, notes, samples_of(&back))
}

#[test]
fn opus_takes_side_surrounds_and_gives_back_the_back_layout() {
    // (channels, WAVE mask with side surrounds, LFE index, Opus's name for it)
    for (channels, mask, lfe, name, words) in [
        (4usize, 0x603u32, None, "quad", "quad"),
        (5, 0x607, None, "5.0-back", "5.0"),
        (6, 0x60F, Some(3), "5.1-back", "5.1"),
    ] {
        let dir = Dir::new(&format!("opus-side-{channels}"));
        let (probe, notes, got) = through_opus(&dir, channels, Some(mask), lfe);
        assert!(probe.contains(name), "{channels}: {probe}");
        assert!(
            notes.contains(&format!(
                "note: {words} with side surrounds written as {words} with back surrounds: \
                 opus names its surround pair as back speakers"
            )),
            "{notes}"
        );
        assert_tones_in_place(&got, channels, lfe, name);
    }
}

#[test]
fn opus_takes_unlabelled_channels_as_its_layout_for_their_count() {
    for (channels, lfe, words) in [
        (3usize, None, "3.0"),
        (4, None, "quad with back surrounds"),
        (6, Some(3), "5.1 with back surrounds"),
        (8, Some(3), "7.1"),
    ] {
        let dir = Dir::new(&format!("opus-unlabelled-{channels}"));
        let (_, notes, got) = through_opus(&dir, channels, None, lfe);
        assert!(
            notes.contains(&format!(
                "note: {channels} unlabelled channels taken as {words}: opus's layout for \
                 {channels} channels"
            )),
            "{notes}"
        );
        assert_tones_in_place(&got, channels, lfe, words);
    }
}

#[test]
fn opus_own_layouts_need_no_note() {
    let dir = Dir::new("opus-own");
    let (_, notes, got) = through_opus(&dir, 6, Some(0x3F), Some(3));
    assert!(
        !notes.contains("surround") && !notes.contains("unlabelled"),
        "{notes}"
    );
    assert_tones_in_place(&got, 6, Some(3), "5.1-back");
}

/// A layout written to `.m4a` (through FLAC when `via_flac`) and back: the notes and whether the
/// decoded samples equal the source's, and the probe of the m4a.
fn through_alac(
    dir: &Dir,
    channels: usize,
    mask: Option<u32>,
    via_flac: bool,
) -> (String, String, Vec<i16>) {
    let (wav_in, flac, m4a, back) = (
        dir.path("in.wav"),
        dir.path("in.flac"),
        dir.path("o.m4a"),
        dir.path("back.wav"),
    );
    let pcm = tones(channels, None);
    std::fs::write(&wav_in, wav(48_000, channels, mask, &pcm)).unwrap();
    let source = if via_flac {
        ok(&["convert", "--force", &wav_in, "-o", &flac]);
        flac
    } else {
        wav_in
    };
    let (_, notes) = ok(&["convert", "--force", &source, "-o", &m4a]);
    let (probe, _) = ok(&["probe", &m4a]);
    ok(&["convert", "--force", &m4a, "-o", &back]);
    let got = samples_of(&back);
    assert_eq!(got, pcm, "ALAC is bit-exact");
    (probe, notes, got)
}

#[test]
fn alac_takes_back_surrounds_and_gives_back_its_side_layout_bit_for_bit() {
    for (channels, mask, name, words) in [(5usize, 0x37u32, "5.0", "5.0"), (6, 0x3F, "5.1", "5.1")]
    {
        for via_flac in [false, true] {
            let dir = Dir::new(&format!("alac-back-{channels}-{via_flac}"));
            let (probe, notes, got) = through_alac(&dir, channels, Some(mask), via_flac);
            assert!(probe.contains(name) && !probe.contains("-back"), "{probe}");
            assert!(
                notes.contains(&format!(
                    "note: {words} with back surrounds written as {words} with side surrounds: \
                     alac names its surround pair as side speakers"
                )),
                "{notes}"
            );
            assert_tones_in_place(&got, channels, None, name);
        }
    }
}

#[test]
fn alac_takes_unlabelled_channels_as_its_layout_for_their_count() {
    for (channels, words) in [
        (3usize, "3.0"),
        (4, "4.0"),
        (5, "5.0 with side surrounds"),
        (6, "5.1 with side surrounds"),
        (7, "6.1"),
    ] {
        let dir = Dir::new(&format!("alac-unlabelled-{channels}"));
        let (_, notes, _) = through_alac(&dir, channels, None, false);
        assert!(
            notes.contains(&format!(
                "note: {channels} unlabelled channels taken as {words}: alac's layout for \
                 {channels} channels"
            )),
            "{notes}"
        );
    }
    let dir = Dir::new("alac-unlabelled-8");
    let (_, notes, _) = through_alac(&dir, 8, None, false);
    assert!(
        notes.contains("note: 8 unlabelled channels taken as alac's own 8-channel layout"),
        "{notes}"
    );
}

#[test]
fn alac_own_layouts_need_no_note() {
    let dir = Dir::new("alac-own");
    let (_, notes, _) = through_alac(&dir, 6, Some(0x60F), false);
    assert!(
        !notes.contains("surround") && !notes.contains("unlabelled"),
        "{notes}"
    );
}

#[test]
fn what_is_not_a_surround_rename_is_still_refused_with_advice_that_works() {
    let dir = Dir::new("refusals");
    let wav_of = |name: &str, channels: usize, mask: u32| {
        let p = dir.path(name);
        let silence = vec![0i16; 4_800 * channels];
        std::fs::write(&p, wav(48_000, channels, Some(mask), &silence)).unwrap();
        p
    };
    let (opus, m4a) = (dir.path("o.opus"), dir.path("o.m4a"));
    // Opus: 4.0 is front centre and back centre, not a quad.
    let e = refused(&["convert", &wav_of("four.wav", 4, 0x107), "-o", &opus]);
    assert!(
        e.contains(
            "opus stores 4 channels in one layout only (quad) and this stream is 4.0; use \
             --channels stereo to mix down, or a mix filter with a matrix puts the channels in \
             that layout"
        ),
        "{e}"
    );
    // ALAC: quad is not its 4.0, and no standard mix reaches it.
    let e = refused(&["convert", &wav_of("quad.wav", 4, 0x33), "-o", &m4a]);
    assert!(
        e.contains(
            "alac stores 4 channels in one layout only (4.0) and this stream is quad; no \
             standard mix reaches it, so a mix filter with a matrix is the way"
        ),
        "{e}"
    );
    // ALAC's 8 channels have front-of-centre speakers: they are not standard 7.1.
    let e = refused(&["convert", &wav_of("seven1.wav", 8, 0x63F), "-o", &m4a]);
    assert!(
        e.contains("this stream is 7.1") && e.contains("no --channels name selects it"),
        "{e}"
    );
    // Both surround pairs, a different LFE or centre set: never silently reordered or mixed.
    let e = refused(&["convert", &wav_of("lfe.wav", 5, 0x60B), "-o", &opus]);
    assert!(
        e.contains("opus stores 5 channels in one layout only (5.0-back)"),
        "{e}"
    );
    let e = refused(&["convert", &wav_of("six1.wav", 7, 0x13F), "-o", &m4a]);
    assert!(
        e.contains("alac stores 7 channels in one layout only (6.1)"),
        "{e}"
    );
    // The advice still works.
    ok(&[
        "convert",
        &wav_of("four2.wav", 4, 0x107),
        "-o",
        &opus,
        "--channels",
        "stereo",
    ]);
}

#[test]
fn flac_wav_and_aiff_are_untouched_and_never_note_a_surround_name() {
    let dir = Dir::new("others");
    let wav_in = dir.path("in.wav");
    std::fs::write(&wav_in, wav(48_000, 6, Some(0x60F), &tones(6, Some(3)))).unwrap();
    for ext in ["flac", "wav", "aiff"] {
        let out = dir.path(&format!("o.{ext}"));
        let (_, notes) = ok(&["convert", "--force", &wav_in, "-o", &out]);
        assert!(!notes.contains("surround"), "{ext}: {notes}");
        let (probe, _) = ok(&["probe", &out]);
        // AIFF has no mask to keep: its six channels read back unlabelled, as before.
        if ext != "aiff" {
            assert!(
                probe.contains("5.1") && !probe.contains("5.1-back"),
                "{ext}: {probe}"
            );
        }
    }
}
