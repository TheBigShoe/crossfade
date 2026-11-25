//! `cf_feed_fill` and `cf_feed_fill_planar` neither allocate nor free, whatever the feed is
//! doing: playing, starting a new segment after a seek, running dry, ending, paused. Counted
//! by this binary's global allocator on the calling thread only.
#![allow(clippy::undocumented_unsafe_blocks)]

mod common;

use common::{Dir, c, write_wav};
use crossfade_ffi::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static CALLS: Cell<usize> = const { Cell::new(0) };
}

fn note() {
    // `try_with`: the thread's slots may be gone while it exits.
    if ARMED.try_with(Cell::get).unwrap_or(false) {
        let _ = CALLS.try_with(|c| c.set(c.get() + 1));
    }
}

// SAFETY: every call is passed straight to the system allocator.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: the caller's layout, as given.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: as above.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        // SAFETY: as above.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note();
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Runs `f` and counts the allocator calls it made on this thread.
fn counted(f: impl FnOnce()) -> usize {
    CALLS.with(|c| c.set(0));
    ARMED.with(|a| a.set(true));
    f();
    ARMED.with(|a| a.set(false));
    CALLS.with(Cell::get)
}

#[test]
fn filling_never_touches_the_allocator() {
    let dir = Dir::new("realtime");
    let path = write_wav(&dir, "a.wav", 44_100, 2, 44_100);
    let mut player = std::ptr::null_mut();
    assert_eq!(unsafe { cf_player_new(48_000, 2, &mut player) }, CF_OK);
    let mut feed = std::ptr::null_mut();
    assert_eq!(unsafe { cf_player_take_feed(player, &mut feed) }, CF_OK);
    let mut buf = vec![0.0f32; 2 * 4096];
    let mut left = vec![0.0f32; 4096];
    let mut right = vec![0.0f32; 4096];
    let planes = [left.as_mut_ptr(), right.as_mut_ptr()];
    let settle = || assert_eq!(unsafe { cf_player_settle(player) }, CF_OK);
    let mut pull = |frames: usize| {
        let calls = counted(|| {
            let s = unsafe { cf_feed_fill(feed, buf.as_mut_ptr(), frames * 2) };
            assert_eq!(s, CF_OK);
            let s = unsafe { cf_feed_fill_planar(feed, planes.as_ptr(), 2, frames) };
            assert_eq!(s, CF_OK);
            // Misuse is refused without a message, so without an allocation.
            let s = unsafe { cf_feed_fill(std::ptr::null_mut(), buf.as_mut_ptr(), 2) };
            assert_eq!(s, CF_ERR_NULL_ARGUMENT);
        });
        assert_eq!(calls, 0, "{frames} frames");
    };

    // Nothing loaded.
    pull(512);
    let p = c(&path);
    assert_eq!(unsafe { cf_player_load(player, p.as_ptr()) }, CF_OK);
    settle();
    // Loaded, not playing; then playing, which adopts the first segment.
    pull(512);
    assert_eq!(unsafe { cf_player_play(player) }, CF_OK);
    settle();
    for _ in 0..8 {
        pull(4096);
        settle();
    }
    // A new segment after a seek, and one before the worker has filled it.
    assert_eq!(unsafe { cf_player_seek(player, 10_000) }, CF_OK);
    pull(256);
    settle();
    pull(1024);
    // To the end and past it.
    for _ in 0..16 {
        settle();
        pull(4096);
    }
    assert_eq!(unsafe { cf_player_pause(player) }, CF_OK);
    pull(128);
    assert_eq!(unsafe { cf_player_destroy(player) }, CF_OK);
    pull(64);
    unsafe { cf_feed_destroy(feed) };
}
