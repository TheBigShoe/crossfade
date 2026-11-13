//! A manual check of the real output device, never run by tests: plays a file (or a generated
//! tone) through the app's own `DeviceSink` and `Player` for a few seconds and prints what the
//! device pulled, so a person (or a log) can confirm the callback runs at the device's rate
//! with no underruns.
//!
//! `cargo run -p crossfade-ui --example audio_check -- [FILE] [SECONDS]`

use std::time::{Duration, Instant};

use crossfade_app::FsIo;
use crossfade_app::player::{AudioSink, Player, State};
use crossfade_core::Limits;
use crossfade_ui::audio_out::default_output;

/// A quiet 440 Hz stereo tone, 16-bit, `seconds` long, as WAV bytes.
fn tone(rate: u32, seconds: u32) -> Vec<u8> {
    let frames = rate * seconds;
    let data_len = frames * 4;
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * 4).to_le_bytes());
    v.extend_from_slice(&4u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        let t = f64::from(i) / f64::from(rate);
        let s = ((t * 440.0 * std::f64::consts::TAU).sin() * 0.05 * 32767.0) as i16;
        v.extend_from_slice(&s.to_le_bytes());
        v.extend_from_slice(&s.to_le_bytes());
    }
    v
}

fn main() {
    let mut args = std::env::args().skip(1);
    let file = args.next().filter(|f| !f.is_empty());
    let seconds: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(3.0);
    let sink = match default_output() {
        Ok(sink) => sink,
        Err(why) => {
            eprintln!("no device: {why}");
            std::process::exit(1);
        }
    };
    println!("device: {}", sink.describe());
    let format = sink.format();
    let path = match file {
        Some(path) => path,
        None => {
            // 44.1 kHz on purpose, so a 48 kHz device exercises the resampler.
            let path = std::env::temp_dir().join("crossfade-audio-check.wav");
            std::fs::write(&path, tone(44_100, 10)).expect("write the tone");
            path.to_string_lossy().into_owned()
        }
    };
    let mut player = Player::new(
        Box::new(sink),
        Box::new(FsIo::new()),
        Limits::default(),
        || {},
    )
    .expect("the device takes the feed");
    player.load(path.as_str());
    player.settle();
    println!("loaded {path}: {:?}", player.state());
    let started = Instant::now();
    player.play();
    let mut last = (0u64, Instant::now());
    while started.elapsed() < Duration::from_secs_f64(seconds) {
        std::thread::sleep(Duration::from_millis(500));
        let (pulled, at) = (player.pulled(), Instant::now());
        let rate = (pulled - last.0) as f64 / at.duration_since(last.1).as_secs_f64();
        println!(
            "t={:.1}s pulled={pulled} (~{rate:.0} frames/s, device {} Hz) position={} underruns={} {:?}",
            started.elapsed().as_secs_f64(),
            format.sample_rate,
            player.position(),
            player.underruns(),
            player.state(),
        );
        last = (pulled, at);
        if player.state() == State::Ended {
            break;
        }
    }
    player.pause();
    println!(
        "done: pulled={} position={} underruns={}",
        player.pulled(),
        player.position(),
        player.underruns()
    );
}
