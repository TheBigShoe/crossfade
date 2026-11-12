//! Writes the WAV files the MP4 fixtures are encoded from: `cargo run -p crossfade-format
//! --example mp4_fixtures -- <dir>`, then run `tests/fixtures/mp4/regen.sh <dir>`.

#[path = "../tests/support/mp4_signals.rs"]
mod mp4_signals;

fn main() {
    let dir = std::env::args().nth(1).expect("usage: mp4_fixtures <dir>");
    std::fs::create_dir_all(&dir).expect("create the output directory");
    for name in mp4_signals::SIGNALS {
        let s = mp4_signals::signal(name);
        let path = format!("{dir}/{name}.wav");
        std::fs::write(&path, s.wav()).expect("write WAV");
        println!(
            "{path}: {} Hz, {} ch, {} bit, {} frames",
            s.rate,
            s.channels,
            s.bits,
            s.frames()
        );
    }
}
