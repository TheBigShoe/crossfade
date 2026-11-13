//! Writes the WAV sources the MP3 fixtures are encoded from: `cargo run -p crossfade-format
//! --example mp3_fixtures -- <dir>`, then run `tests/fixtures/mp3/regen.sh <dir>`.

#[path = "../tests/support/mp3_signals.rs"]
mod mp3_signals;

fn main() {
    let dir = std::env::args().nth(1).expect("usage: mp3_fixtures <dir>");
    std::fs::create_dir_all(&dir).expect("create the output directory");
    for f in mp3_signals::FIXTURES {
        let path = format!("{dir}/{}.wav", f.name);
        std::fs::write(&path, mp3_signals::wav(f)).expect("write WAV");
        println!("{path}\t{}", f.lame);
    }
}
