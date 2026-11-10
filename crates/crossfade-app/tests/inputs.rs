mod common;

use common::{sine_wav, spec};
use crossfade_app::inputs::{InputState, Inputs, probe_path};
use crossfade_core::{ChannelLayout, Limits, SampleFormat};
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
    let InputState::Probed(doc) = probe_path("a.wav", &mut io, &Limits::default()) else {
        panic!("expected Probed");
    };
    assert_eq!(doc.format, "wav");
    assert_eq!(doc.streams[0].frames, Some(100));
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
