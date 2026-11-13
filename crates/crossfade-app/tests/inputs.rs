mod common;

use common::{sine_wav, spec};
use crossfade_app::inputs::{InputState, Inputs, probe_path};
use crossfade_core::{ChannelLayout, Error, Limits, SampleFormat};
use crossfade_testkit::memory::MemoryIo;

fn failed() -> InputState {
    InputState::Failed("x".into())
}

fn names(inputs: &Inputs) -> Vec<&str> {
    inputs.entries().iter().map(|e| e.name.as_str()).collect()
}

#[test]
fn probe_path_reports_a_document() {
    let s = spec(8000, SampleFormat::S16, ChannelLayout::mono());
    let (_, wav) = sine_wav(&s, 100);
    let mut io = MemoryIo::new();
    io.insert("a.wav", wav);
    let InputState::Probed(doc, _) = probe_path("a.wav", &mut io, &Limits::default()) else {
        panic!("expected Probed");
    };
    assert_eq!(doc.format, "wav");
    assert_eq!(doc.streams[0].frames, Some(100));
}

#[test]
fn a_probed_entry_keeps_the_first_stream_for_validation() {
    let s = spec(8000, SampleFormat::S16, ChannelLayout::mono());
    let (_, wav) = sine_wav(&s, 100);
    let mut io = MemoryIo::new();
    io.insert("a.wav", wav);
    let mut inputs = Inputs::new();
    inputs.add("a.wav", probe_path("a.wav", &mut io, &Limits::default()));
    let entry = inputs.selected().unwrap();
    let stream = entry.stream().expect("a probed file has a stream");
    assert_eq!(stream.audio.sample_rate(), 8000);
    assert_eq!(stream.frames, Some(100));
    assert_eq!(entry.document().unwrap().format, "wav");

    inputs.add("bad", failed());
    assert!(inputs.selected().unwrap().stream().is_none());
    assert!(inputs.selected().unwrap().document().is_none());
}

#[test]
fn probe_path_failures_carry_the_engine_message() {
    let mut io = MemoryIo::new();
    io.insert("z.bin", vec![0; 64]);
    assert_eq!(
        probe_path("z.bin", &mut io, &Limits::default()),
        InputState::Failed("unrecognised format".into())
    );
    let InputState::Failed(m) = probe_path("nope.wav", &mut io, &Limits::default()) else {
        panic!("expected Failed");
    };
    assert!(m.contains("nope.wav"), "{m}");
}

#[test]
fn a_new_inputs_list_is_empty_with_nothing_selected() {
    let inputs = Inputs::new();
    assert!(inputs.is_empty());
    assert!(inputs.selected().is_none());
}

#[test]
fn adding_selects_the_new_entry_and_names_it_by_file_name() {
    let mut inputs = Inputs::new();
    inputs.add("/music/a.wav", failed());
    let b = inputs.add("/music/b.wav", failed());
    assert_eq!(names(&inputs), ["a.wav", "b.wav"]);
    assert_eq!(inputs.selected().unwrap().id, b);
    assert_eq!(inputs.selected().unwrap().path, "/music/b.wav");
}

#[test]
fn the_same_path_again_replaces_state_keeps_id_and_selects_it() {
    let mut inputs = Inputs::new();
    let a = inputs.add("a.wav", failed());
    let b = inputs.add("b.wav", failed());
    assert_eq!(inputs.selected().unwrap().id, b);
    let again = inputs.add("a.wav", InputState::Failed("new".into()));
    assert_eq!(again, a);
    assert_eq!(inputs.entries().len(), 2);
    assert_eq!(inputs.selected().unwrap().id, a);
    assert_eq!(inputs.entries()[0].state, InputState::Failed("new".into()));
}

#[test]
fn removing_the_selected_entry_selects_the_next_else_the_previous_else_none() {
    let mut inputs = Inputs::new();
    let a = inputs.add("a", failed());
    let b = inputs.add("b", failed());
    let c = inputs.add("c", failed());
    inputs.select(b);
    inputs.remove(b);
    assert_eq!(inputs.selected().unwrap().id, c);
    inputs.remove(c);
    assert_eq!(inputs.selected().unwrap().id, a);
    inputs.remove(a);
    assert!(inputs.selected().is_none());
    assert!(inputs.is_empty());
}

#[test]
fn removing_another_entry_keeps_the_selection() {
    let mut inputs = Inputs::new();
    let a = inputs.add("a", failed());
    let b = inputs.add("b", failed());
    inputs.remove(a);
    assert_eq!(inputs.selected().unwrap().id, b);
}

#[test]
fn selecting_an_unknown_id_changes_nothing() {
    let mut inputs = Inputs::new();
    let a = inputs.add("a", failed());
    inputs.add("b", failed());
    inputs.remove(a);
    inputs.select(a);
    assert_eq!(inputs.selected().unwrap().name, "b");
    inputs.remove(a);
    assert_eq!(inputs.entries().len(), 1);
}

#[test]
fn ids_are_never_reused() {
    let mut inputs = Inputs::new();
    let a = inputs.add("a", failed());
    inputs.remove(a);
    assert_ne!(inputs.add("a", failed()), a);
}

/// One Ogg page holding `packet`, marked first and last, with a valid checksum.
fn ogg_page(packet: &[u8]) -> Vec<u8> {
    let mut out = b"OggS".to_vec();
    out.extend([0, 0x03]);
    out.extend(0i64.to_le_bytes());
    out.extend(1u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend([0; 4]);
    out.push(1);
    out.push(u8::try_from(packet.len()).unwrap());
    out.extend(packet);
    let mut crc = 0u32;
    for byte in &out {
        crc ^= u32::from(*byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04c1_1db7
            } else {
                crc << 1
            };
        }
    }
    out[22..26].copy_from_slice(&crc.to_le_bytes());
    out
}

#[test]
fn an_ogg_file_that_is_not_flac_is_refused_in_plain_words() {
    let mut io = MemoryIo::new();
    io.insert("song.ogg", ogg_page(b"\x01vorbis\0\0\0\0"));
    io.insert("odd.ogg", ogg_page(b"mystery!"));
    io.insert("talk.opus", ogg_page(b"OpusHead\x01\x02\0\0"));
    io.insert("film.ogv", ogg_page(b"\x80theora\0\0\0\0"));
    let InputState::Failed(opus) = probe_path("talk.opus", &mut io, &Limits::default()) else {
        panic!("expected Failed");
    };
    assert_eq!(
        opus,
        "This Ogg file holds Opus audio. Crossfade recognises Opus but cannot open it yet."
    );
    let InputState::Failed(film) = probe_path("film.ogv", &mut io, &Limits::default()) else {
        panic!("expected Failed");
    };
    assert_eq!(
        film,
        "This Ogg file holds Theora video, which Crossfade cannot open. \
         Inside Ogg it opens FLAC and Vorbis audio."
    );
    let InputState::Failed(vorbis) = probe_path("song.ogg", &mut io, &Limits::default()) else {
        panic!("expected Failed");
    };
    // Vorbis in Ogg is read now: this one-page stub is a broken Vorbis stream, not a refusal.
    assert_eq!(
        vorbis,
        "malformed input: ogg vorbis: identification header too short"
    );
    let InputState::Failed(odd) = probe_path("odd.ogg", &mut io, &Limits::default()) else {
        panic!("expected Failed");
    };
    assert!(
        odd.starts_with("This Ogg file holds audio Crossfade does not recognise"),
        "{odd}"
    );
    assert!(!vorbis.contains("unsupported:") && !odd.contains("--"));
}

#[test]
fn aac_without_the_feature_says_this_build_lacks_it() {
    let engine = Error::Unsupported {
        what: "aac decoding is not part of this build (AAC is patent-encumbered); build with --features aac".into(),
    };
    assert_eq!(
        crossfade_app::inputs::refusal(&engine),
        "This build of Crossfade does not include AAC decoding."
    );
}
