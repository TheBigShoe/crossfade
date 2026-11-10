//! Writes the raw PCM the FLAC fixtures are encoded from: `cargo run -p crossfade-format
//! --example flac_fixtures -- <dir>`, then run `tests/fixtures/flac/regen.sh <dir>`.

#[path = "../tests/support/signals.rs"]
mod signals;

fn main() {
    let dir = std::env::args().nth(1).expect("usage: flac_fixtures <dir>");
    std::fs::create_dir_all(&dir).expect("create the output directory");
    let mut names: Vec<&str> = signals::FIXTURES.iter().map(|&(_, s)| s).collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        let s = signals::signal(name);
        let path = format!("{dir}/{name}.raw");
        std::fs::write(&path, s.raw_pcm()).expect("write raw PCM");
        if !s.bits.is_multiple_of(8) {
            std::fs::write(format!("{dir}/{name}.wav"), s.wav_extensible()).expect("write WAV");
        }
        println!(
            "{path}: {} Hz, {} ch, {} bit, {} frames",
            s.rate,
            s.channels,
            s.bits,
            s.frames()
        );
    }
}
