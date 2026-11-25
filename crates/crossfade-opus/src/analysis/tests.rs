#![allow(clippy::excessive_precision)] // libopus's outputs as it printed them

use super::*;

/// The vowel resonators of the speech-like signal: per vowel, three formants' recursion
/// coefficients (exact in `f32`, so that libopus's harness made the same signal).
const FORMANTS: [[[f32; 2]; 3]; 5] = [
    [
        [1.980_478_9, -0.989_582_66],
        [1.966_762_1, -0.986_995_34],
        [1.880_300_3, -0.980_556_55],
    ],
    [
        [1.988_313, -0.989_582_66],
        [1.898_349_2, -0.986_995_34],
        [1.828_713_9, -0.980_556_55],
    ],
    [
        [1.984_769_3, -0.989_582_66],
        [1.929_598, -0.986_995_34],
        [1.877_018_5, -0.980_556_55],
    ],
    [
        [1.984_02, -0.989_582_66],
        [1.974_953_5, -0.986_995_34],
        [1.882_727_7, -0.980_556_55],
    ],
    [
        [1.988_021_5, -0.989_582_66],
        [1.974_082, -0.986_995_34],
        [1.895_934, -0.980_556_55],
    ],
];

/// Test signals at 48 kHz, built from exact float operations only (so that a C harness over
/// libopus made the very same samples): a 440 Hz tone, noise, a speech-like signal (pitched
/// vowels, fricatives and pauses), silence, bursts of noise, and a stereo tone and noise.
struct Signal {
    kind: &'static str,
    channels: usize,
    n: usize,
    seed: u32,
    osc: [[f32; 2]; 2],
    y1: [f32; 3],
    y2: [f32; 3],
    last: f32,
}

impl Signal {
    fn new(kind: &'static str) -> Self {
        Self {
            kind,
            channels: if kind == "stereo" { 2 } else { 1 },
            n: 0,
            seed: 0x1234_5678,
            osc: [[0.0; 2]; 2],
            y1: [0.0; 3],
            y2: [0.0; 3],
            last: 0.0,
        }
    }

    fn sample(&mut self, ch: usize) -> f32 {
        let n = self.n;
        self.seed = self
            .seed
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        let noise = ((self.seed >> 8) as f32 / 16_777_216.0) * 2.0 - 1.0;
        match (self.kind, ch) {
            ("silence", _) => 0.0,
            ("noise", _) => 0.3 * noise,
            ("tone", _) | ("stereo", 0) => {
                let st = &mut self.osc[ch];
                let mut y = 1.996_683_6 * st[0] - st[1];
                if n == 0 {
                    y = 0.0;
                }
                if n == 1 {
                    y = 0.057_564_02;
                }
                st[1] = st[0];
                st[0] = y;
                0.5 * y
            }
            ("stereo", _) => 0.2 * noise,
            ("speech", _) => {
                const PERIODS: [usize; 5] = [343, 300, 320, 280, 360];
                const GAINS: [f32; 3] = [0.06, 0.03, 0.015];
                let syl = n / 12_000;
                let pos = n % 12_000;
                let pause = n % 48_000 >= 40_800;
                let period = PERIODS[syl % 5] + pos / 2400;
                let mut v = 0.0f32;
                if !pause && pos < 8160 {
                    let e1 = pos as f32 / 960.0;
                    let e2 = (8160 - pos) as f32 / 960.0;
                    let env = e1.min(1.0) * e2.min(1.0);
                    let x = if pos.is_multiple_of(period) { env } else { 0.0 };
                    for k in 0..3 {
                        let [a1, a2] = FORMANTS[syl % 5][k];
                        let y = x + a1 * self.y1[k] + a2 * self.y2[k];
                        self.y2[k] = self.y1[k];
                        self.y1[k] = y;
                        v += GAINS[k] * y;
                    }
                } else if !pause && pos < 10_560 {
                    v = 0.08 * (noise - 0.7 * self.last);
                }
                self.last = noise;
                v + 0.001 * noise
            }
            ("transient", _) => {
                let pos = n % 19_200;
                if pos < 480 {
                    0.8 * noise * (1.0 - pos as f32 / 480.0)
                } else {
                    0.0
                }
            }
            _ => unreachable!(),
        }
    }

    /// The next 20 ms, interleaved.
    fn frame(&mut self) -> Vec<f32> {
        let mut out = Vec::with_capacity(960 * self.channels);
        for _ in 0..960 {
            for ch in 0..self.channels {
                let v = self.sample(ch);
                out.push(v);
            }
            self.n += 1;
        }
        out
    }
}

/// The analysis of `frames` 20 ms frames of `kind`, as the encoder reads it frame by frame.
fn analyse(kind: &'static str, frames: usize) -> Vec<AnalysisInfo> {
    let mut sig = Signal::new(kind);
    let mut an = TonalityAnalysis::default();
    (0..frames)
        .map(|_| an.run(&sig.frame(), sig.channels, 960))
        .collect()
}

/// Frames of libopus 1.5.2's analysis of the same signals (`run_analysis` in its float build,
/// gcc -O2 -ffp-contract=off, 20 ms at a time; a development check, not run here): frame,
/// tonality, tonality slope, noisiness, activity, music probability, its min and max,
/// bandwidth, activity probability, max pitch ratio, and the first eight leakage boosts.
type Expected = (usize, [f32; 10], i32, [u8; 8]);

fn check(kind: &'static str, expected: &[Expected], tol: f32, prob_tol: f32) {
    let got = analyse(kind, 100);
    for &(frame, v, bandwidth, leak) in expected {
        let g = &got[frame];
        assert!(g.valid, "{kind} frame {frame} is not valid");
        let ours = [
            g.tonality,
            g.tonality_slope,
            g.noisiness,
            g.activity,
            g.music_prob,
            g.music_prob_min,
            g.music_prob_max,
            0.0,
            g.activity_probability,
            g.max_pitch_ratio,
        ];
        let names = [
            "tonality",
            "slope",
            "noisiness",
            "activity",
            "music",
            "music min",
            "music max",
            "",
            "activity prob",
            "pitch ratio",
        ];
        for k in 0..10 {
            let t = if (4..=8).contains(&k) { prob_tol } else { tol };
            assert!(
                (ours[k] - v[k]).abs() <= t,
                "{kind} frame {frame} {}: {} against libopus {}",
                names[k],
                ours[k],
                v[k]
            );
        }
        assert_eq!(g.bandwidth, bandwidth, "{kind} frame {frame} bandwidth");
        for b in 0..8 {
            assert!(
                (i32::from(g.leak_boost[b]) - i32::from(leak[b])).abs() <= 1,
                "{kind} frame {frame} leak boost {b}: {:?} against {leak:?}",
                &g.leak_boost[..8]
            );
        }
    }
}

const TONE: [Expected; 3] = [
    (
        20,
        [
            0.726885498,
            -0.0700771436,
            0.879477322,
            0.886619925,
            0.99973768,
            0.91488409,
            0.999973774,
            0.0,
            0.915609777,
            1.0,
        ],
        20,
        [233, 73, 255, 187, 255, 250, 191, 107],
    ),
    (
        60,
        [
            0.725775957,
            -0.0424971581,
            0.887820959,
            0.894488752,
            0.999714255,
            0.912973046,
            0.999971449,
            0.0,
            0.963725269,
            1.0,
        ],
        20,
        [233, 73, 255, 187, 255, 250, 191, 107],
    ),
    (
        99,
        [
            0.673945487,
            -0.078125082,
            0.900410771,
            0.906137049,
            0.999681354,
            0.911787987,
            0.999968112,
            0.0,
            0.976577342,
            1.0,
        ],
        20,
        [215, 55, 255, 188, 255, 253, 195, 110],
    ),
];
const NOISE: [Expected; 2] = [
    (
        10,
        [
            0.123058029,
            0.025244236,
            0.037813291,
            0.581230521,
            0.00608313084,
            0.00489015132,
            0.171427116,
            0.0,
            0.00214144588,
            0.194400087,
        ],
        20,
        [0, 0, 0, 0, 0, 0, 0, 0],
    ),
    (
        50,
        [
            0.136865079,
            -0.018755056,
            -0.0107439114,
            0.668076873,
            0.0145645738,
            0.0107558798,
            0.0201466084,
            0.0,
            0.000578582287,
            0.172790945,
        ],
        20,
        [0, 0, 0, 0, 0, 0, 0, 0],
    ),
];
const SPEECH: [Expected; 3] = [
    (
        13,
        [
            0.176580831,
            -0.0666518658,
            0.206189081,
            0.748865604,
            0.0496115386,
            0.00496115535,
            0.904961109,
            0.0,
            0.998738587,
            1.0,
        ],
        20,
        [0, 0, 0, 0, 0, 0, 0, 0],
    ),
    (
        27,
        [
            0.416464806,
            -0.132638544,
            0.705344021,
            0.94592768,
            0.542329848,
            0.0542329848,
            0.722801626,
            0.0,
            0.999830008,
            1.0,
        ],
        20,
        [0, 0, 0, 0, 0, 0, 0, 0],
    ),
    (
        70,
        [
            0.7277776,
            0.0585218109,
            0.220477283,
            0.810048997,
            0.957524359,
            0.0957524776,
            0.995752454,
            0.0,
            0.999929309,
            1.0,
        ],
        20,
        [0, 0, 0, 0, 0, 0, 0, 0],
    ),
];
const TRANSIENT: [Expected; 2] = [
    (
        30,
        [
            0.0486283116,
            0.00120555947,
            0.000655433279,
            0.500327706,
            0.184318513,
            0.161302447,
            0.207334578,
            0.0,
            0.255734086,
            5.08312951e-05,
        ],
        18,
        [0, 0, 0, 0, 0, 0, 0, 0],
    ),
    (
        80,
        [
            0.075493224,
            -0.0584889725,
            0.0197874289,
            0.509893715,
            0.706747353,
            0.683512449,
            0.804358482,
            0.0,
            0.258165359,
            0.145740539,
        ],
        20,
        [0, 0, 0, 0, 0, 0, 0, 0],
    ),
];
const STEREO: [Expected; 2] = [
    (
        30,
        [
            0.237360403,
            -0.141927049,
            0.0808040798,
            0.534240186,
            0.0309926867,
            0.0119299404,
            0.0312094279,
            0.0,
            0.00240823627,
            1.0,
        ],
        20,
        [0, 0, 44, 44, 0, 0, 0, 0],
    ),
    (
        66,
        [
            0.174862906,
            -0.11058706,
            0.100973025,
            0.593559384,
            0.0819033682,
            0.0662500039,
            0.126583502,
            0.0,
            0.0014513135,
            1.0,
        ],
        20,
        [0, 0, 53, 53, 0, 0, 0, 0],
    ),
];

#[test]
fn a_tone_is_tonal_music_as_libopus_finds() {
    // A pure tone's quiet bins hold only the FFTs' rounding, whose phases differ between
    // libopus's FFT and ours: its tonality is libopus's within a few hundredths.
    check("tone", &TONE, 0.05, 0.01);
}

#[test]
fn noise_is_neither_tonal_nor_music_as_libopus_finds() {
    check("noise", &NOISE, 1e-4, 1e-4);
}

#[test]
fn a_speech_like_signal_is_analysed_as_libopus_does() {
    check("speech", &SPEECH, 1e-4, 1e-4);
}

#[test]
fn bursts_are_analysed_as_libopus_does() {
    check("transient", &TRANSIENT, 1e-4, 1e-4);
}

#[test]
fn stereo_is_analysed_on_its_downmix_as_libopus_does() {
    check("stereo", &STEREO, 1e-4, 1e-4);
}

#[test]
fn digital_silence_is_never_valid() {
    assert!(analyse("silence", 60).iter().all(|a| !a.valid));
}

fn finite(a: &AnalysisInfo) -> bool {
    [
        a.tonality,
        a.tonality_slope,
        a.noisiness,
        a.activity,
        a.music_prob,
        a.music_prob_min,
        a.music_prob_max,
        a.activity_probability,
        a.max_pitch_ratio,
    ]
    .iter()
    .all(|v| v.is_finite())
        && (0..=20).contains(&a.bandwidth)
}

#[test]
fn hostile_input_never_panics_and_gives_finite_info() {
    for size in [120, 240, 480, 960, 1920, 2880] {
        for channels in [1, 2] {
            let mut an = TonalityAnalysis::default();
            let mut sig = Signal::new("noise");
            let hostile = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30, -3e38, 0.0];
            for (k, &bad) in hostile.iter().cycle().take(30).enumerate() {
                let mut pcm: Vec<f32> = (0..size * channels).map(|_| sig.sample(0)).collect();
                if k % 3 == 0 {
                    let at = k % pcm.len();
                    pcm[at] = bad;
                } else if k % 3 == 1 {
                    pcm.fill(bad);
                }
                let a = an.run(&pcm, channels, size);
                assert!(finite(&a), "{size} {channels} {k}: {a:?}");
            }
        }
    }
}

#[test]
fn reset_forgets() {
    let mut sig = Signal::new("speech");
    let frames: Vec<Vec<f32>> = (0..20).map(|_| sig.frame()).collect();
    let mut an = TonalityAnalysis::default();
    let fresh: Vec<AnalysisInfo> = frames.iter().map(|f| an.run(f, 1, 960)).collect();
    let mut noise = Signal::new("noise");
    for _ in 0..7 {
        an.run(&noise.frame(), 1, 960);
    }
    an.reset();
    let again: Vec<AnalysisInfo> = frames.iter().map(|f| an.run(f, 1, 960)).collect();
    assert_eq!(fresh, again);
}
