//! The signals the MP3 fixtures were encoded from (`tests/fixtures/mp3/README.md`): the tests
//! regenerate them to check gapless length and alignment.
// Included by a test and an example, each of which uses a different part.
#![allow(dead_code, unreachable_pub)]

use crossfade_core::{AudioSpec, ChannelLayout, SampleFormat};
use crossfade_testkit::signal::{Signal, render};

/// One fixture: file name, sample rate, channels, length in sample frames, and the `lame`
/// options after `--quiet`.
pub struct Fixture {
    pub name: &'static str,
    pub rate: u32,
    pub channels: u16,
    pub frames: usize,
    pub lame: &'static str,
}

pub const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "cbr128",
        rate: 44100,
        channels: 2,
        frames: 22050,
        lame: "-b 128",
    },
    Fixture {
        name: "cbr320",
        rate: 44100,
        channels: 2,
        frames: 22050,
        lame: "-b 320",
    },
    Fixture {
        name: "vbr",
        rate: 44100,
        channels: 2,
        frames: 22050,
        lame: "-V 2",
    },
    Fixture {
        name: "notag",
        rate: 44100,
        channels: 2,
        frames: 22050,
        lame: "-t -b 128",
    },
    Fixture {
        name: "mpeg1-32k",
        rate: 32000,
        channels: 1,
        frames: 32000,
        lame: "-b 32 -m m --resample 32",
    },
    Fixture {
        name: "mpeg2",
        rate: 22050,
        channels: 2,
        frames: 22050,
        lame: "-b 64 --resample 22.05",
    },
    Fixture {
        name: "mpeg2-8k",
        rate: 16000,
        channels: 1,
        frames: 16000,
        lame: "-b 8 -m m --resample 16",
    },
    Fixture {
        name: "mpeg25",
        rate: 8000,
        channels: 1,
        frames: 8000,
        lame: "-b 8 -m m --resample 8",
    },
    Fixture {
        name: "free",
        rate: 44100,
        channels: 2,
        frames: 22050,
        lame: "--freeformat -b 200",
    },
    Fixture {
        name: "tagged",
        rate: 44100,
        channels: 2,
        frames: 22050,
        lame: "-b 128 --add-id3v2 --id3v2-utf16 --tt Title --ta Artist --tl Album --ty 2024 --tn 3/9 --tg Rock --tc comment",
    },
];

pub fn fixture(name: &str) -> &'static Fixture {
    FIXTURES
        .iter()
        .find(|f| f.name == name)
        .expect("known fixture")
}

/// The source, interleaved 16-bit: a sweep from 100 Hz to a quarter of the rate on the left
/// (and on a mono stream), a 440 Hz tone on the right.
pub fn source(f: &Fixture) -> Vec<i16> {
    let spec = AudioSpec::new(f.rate, SampleFormat::F32, ChannelLayout::mono()).unwrap();
    let sweep = Signal::Sweep {
        from_hz: 100.0,
        to_hz: f64::from(f.rate) / 4.0,
        amplitude: 0.5,
    };
    let tone = Signal::Sine {
        hz: 440.0,
        amplitude: 0.4,
    };
    let left = render(&sweep, &spec, f.frames).unwrap();
    let right = render(&tone, &spec, f.frames).unwrap();
    let q = |v: f64| (v * 32767.0).round() as i16;
    if f.channels == 1 {
        left.iter().map(|&v| q(v)).collect()
    } else {
        left.iter()
            .zip(&right)
            .flat_map(|(&l, &r)| [q(l), q(r)])
            .collect()
    }
}

/// A 16-bit PCM WAV file of `source(f)`.
pub fn wav(f: &Fixture) -> Vec<u8> {
    let data: Vec<u8> = source(f).iter().flat_map(|s| s.to_le_bytes()).collect();
    let ch = f.channels;
    let mut w = b"RIFF".to_vec();
    w.extend((36 + data.len() as u32).to_le_bytes());
    w.extend(b"WAVEfmt ");
    w.extend(16u32.to_le_bytes());
    w.extend(1u16.to_le_bytes());
    w.extend(ch.to_le_bytes());
    w.extend(f.rate.to_le_bytes());
    w.extend((f.rate * 2 * u32::from(ch)).to_le_bytes());
    w.extend((2 * ch).to_le_bytes());
    w.extend(16u16.to_le_bytes());
    w.extend(b"data");
    w.extend((data.len() as u32).to_le_bytes());
    w.extend(data);
    w
}
