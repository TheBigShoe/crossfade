//! Seeking an MP3 file through `DecodeSession` is exact: seek-then-decode equals
//! decode-then-slice, bit for bit, on the timeline (gapless trim included), for the MP3
//! fixtures of every MPEG version, VBR and free format.

use crossfade_core::{AudioFrame, Limits, Samples};
use crossfade_graph::DecodeSession;
use crossfade_testkit::memory::MemoryIo;

fn session(name: &str) -> DecodeSession {
    let path = format!(
        "{}/../crossfade-format/tests/fixtures/mp3/{name}.mp3",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut io = MemoryIo::new();
    io.insert("in.mp3", std::fs::read(path).unwrap());
    DecodeSession::open("in.mp3", &mut io, &Limits::default()).unwrap()
}

fn rest(s: &mut DecodeSession) -> Vec<f32> {
    let mut f = AudioFrame::new(&s.stream().audio);
    let mut out = Vec::new();
    while s.next_frame(&mut f).unwrap() {
        let Samples::F32(v) = &f.samples else {
            unreachable!()
        };
        out.extend_from_slice(v);
    }
    out
}

#[test]
fn seek_then_decode_equals_decode_then_slice_through_a_decode_session() {
    for name in [
        "cbr128",
        "cbr320",
        "vbr",
        "mpeg1-32k",
        "mpeg2",
        "mpeg2-8k",
        "mpeg25",
        "free",
    ] {
        let mut s = session(name);
        let ch = usize::from(s.stream().audio.layout().channels());
        let whole = rest(&mut s);
        let frames = (whole.len() / ch) as u64;
        assert_eq!(s.stream().frames, Some(frames), "{name}");
        for target in [
            0,
            1,
            575,
            1152,
            frames / 3,
            frames / 2 + 7,
            frames - 1,
            frames,
            40,
            frames / 3,
        ] {
            s.seek(target).unwrap();
            assert_eq!(s.position(), target, "{name}");
            let got = rest(&mut s);
            let want = &whole[target as usize * ch..];
            assert_eq!(got.len(), want.len(), "{name} {target}");
            assert!(
                got.iter()
                    .zip(want)
                    .all(|(a, b)| a.to_bits() == b.to_bits()),
                "{name}: seek to {target} differs"
            );
        }
    }
}
