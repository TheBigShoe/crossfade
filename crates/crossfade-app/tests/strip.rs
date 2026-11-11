mod common;

use common::{spec, wav_bytes};
use crossfade_app::analysis::{Analyzer, State};
use crossfade_app::strip::{Cache, Strip, Wake};
use crossfade_core::{ChannelLayout, Limits, SampleFormat};
use crossfade_graph::CancelToken;
use crossfade_testkit::memory::MemoryIo;
use std::sync::mpsc;

const RATE: u32 = 8000;

fn wav(frames: usize) -> Vec<u8> {
    let pcm: Vec<u8> = (0..frames * 2)
        .flat_map(|i| (((i * 37) % 20_000) as i16).to_le_bytes())
        .collect();
    wav_bytes(
        &spec(RATE, SampleFormat::S16, ChannelLayout::stereo()),
        &pcm,
    )
}

fn io(path: &str, frames: usize) -> Box<MemoryIo> {
    let mut io = MemoryIo::new();
    io.insert(path, wav(frames));
    Box::new(io)
}

fn finished(path: &str, frames: usize) -> Analyzer {
    let a = Analyzer::start(path, io(path, frames), Limits::default(), |_| {});
    assert_eq!(a.wait().state, State::Done);
    a
}

#[test]
fn the_cache_drops_the_least_recently_used_over_its_cap() {
    let mut c = Cache::new(100);
    c.insert("a".into(), 40, 1);
    c.insert("b".into(), 40, 2);
    c.insert("c".into(), 40, 3);
    assert_eq!(c.keys(), ["b", "c"]);
    assert_eq!(c.bytes(), 80);
    // Reading one makes it the newest.
    let b = c.take("b").unwrap();
    c.insert("b".into(), 40, b);
    c.insert("d".into(), 40, 4);
    assert_eq!(c.keys(), ["b", "d"]);
}

#[test]
fn an_entry_larger_than_the_cap_is_not_kept_and_replaces_nothing() {
    let mut c = Cache::new(100);
    c.insert("a".into(), 60, 1);
    c.insert("huge".into(), 101, 2);
    assert_eq!(c.keys(), ["a"]);
    c.insert("a".into(), 70, 3);
    assert_eq!((c.len(), c.bytes()), (1, 70));
    assert_eq!(c.take("a"), Some(3));
    assert_eq!(c.take("a"), None);
}

#[test]
fn selecting_a_file_starts_its_analysis() {
    let mut strip = Strip::new(1 << 20);
    strip.select(Some("a.wav"), true, |p| finished(p, 4000));
    assert_eq!(strip.path(), Some("a.wav"));
    assert!(strip.analyzer().is_some());
}

#[test]
fn with_automatic_analysis_off_nothing_starts_until_asked() {
    let mut strip = Strip::new(1 << 20);
    strip.select(Some("a.wav"), false, |_| panic!("must not start"));
    assert_eq!(strip.path(), Some("a.wav"));
    assert!(strip.analyzer().is_none());
    strip.analyse(|p| finished(p, 4000));
    assert!(strip.analyzer().is_some());
}

#[test]
fn selecting_nothing_clears_the_strip() {
    let mut strip = Strip::new(1 << 20);
    strip.select(Some("a.wav"), true, |p| finished(p, 4000));
    strip.select(None, true, |_| panic!("no file"));
    assert_eq!(strip.path(), None);
    assert!(strip.analyzer().is_none());
}

#[test]
fn a_finished_analysis_is_kept_and_reselecting_is_instant() {
    let mut strip = Strip::new(1 << 20);
    strip.select(Some("a.wav"), true, |p| finished(p, 4000));
    strip.select(Some("b.wav"), true, |p| finished(p, 2000));
    assert!(strip.is_cached("a.wav"));
    // Same selection again starts nothing.
    strip.select(Some("b.wav"), true, |_| panic!("already current"));
    strip.select(Some("a.wav"), true, |_| panic!("served from the cache"));
    assert_eq!(strip.analyzer().unwrap().snapshot().state, State::Done);
    assert!(!strip.is_cached("a.wav"));
    assert!(strip.is_cached("b.wav"));
}

#[test]
fn the_cache_obeys_its_memory_cap() {
    let one = finished("a.wav", 40_000);
    let bytes = one.with(|a| a.pyramid.heap_bytes());
    let mut strip = Strip::new(bytes + bytes / 2);
    strip.select(Some("a.wav"), true, |_| one);
    strip.select(Some("b.wav"), true, |p| finished(p, 40_000));
    strip.select(Some("c.wav"), true, |p| finished(p, 40_000));
    assert!(!strip.is_cached("a.wav"));
    assert!(strip.is_cached("b.wav"));
}

#[test]
fn changing_the_selection_cancels_a_running_analysis_and_does_not_keep_it() {
    let token = CancelToken::new();
    let (hold, held) = mpsc::channel::<()>();
    let (reached, at_gate) = mpsc::channel::<()>();
    let mut strip = Strip::new(1 << 20);
    strip.select(Some("slow.wav"), true, |p| {
        let held = std::sync::Mutex::new(held);
        let reached = std::sync::Mutex::new(reached);
        Analyzer::start_with_cancel(
            p,
            io(p, 80_000),
            Limits::default(),
            token.clone(),
            move |snap| {
                // Park the worker mid-file so the analysis is certainly still running.
                if snap.state == State::Running {
                    let _ = reached.lock().unwrap().send(());
                    let _ = held.lock().unwrap().recv();
                }
            },
        )
    });
    at_gate.recv().unwrap();
    assert_eq!(strip.analyzer().unwrap().snapshot().state, State::Running);
    strip.select(Some("next.wav"), true, |p| finished(p, 1000));
    assert!(token.is_cancelled());
    assert!(!strip.is_cached("slow.wav"));
    drop(hold);
}

#[test]
fn a_failed_analysis_is_not_kept() {
    let mut strip = Strip::new(1 << 20);
    strip.select(Some("bad.wav"), true, |p| {
        let mut io = MemoryIo::new();
        io.insert(p, b"not audio".to_vec());
        let a = Analyzer::start(p, Box::new(io), Limits::default(), |_| {});
        assert!(matches!(a.wait().state, State::Failed(_)));
        a
    });
    strip.select(Some("b.wav"), true, |p| finished(p, 1000));
    assert!(!strip.is_cached("bad.wav"));
}

#[test]
fn a_new_selection_starts_from_a_fitted_view() {
    let mut strip = Strip::new(1 << 20);
    strip.select(Some("a.wav"), true, |p| finished(p, 4000));
    strip.view_mut().set_width(100);
    strip.view_mut().set_source(4000, RATE);
    strip.view_mut().zoom(4.0, 0.0);
    assert!(!strip.view().is_fit());
    strip.select(Some("b.wav"), true, |p| finished(p, 4000));
    strip.view_mut().set_source(4000, RATE);
    assert!(strip.view().is_fit());
}

#[test]
fn wake_ups_coalesce_until_acknowledged() {
    let w = Wake::default();
    assert!(w.request());
    assert!(!w.request());
    assert!(!w.request());
    w.acknowledge();
    assert!(w.request());
}
