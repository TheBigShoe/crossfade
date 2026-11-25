//! The preview player through the C functions: the native app pulls samples into its own
//! buffers as a device callback would, and gets what the app layer's virtual sink gets.
#![allow(clippy::undocumented_unsafe_blocks)]

mod common;

use common::{Dir, c, json_out, last_error, write_wav};
use crossfade_app::FsIo;
use crossfade_app::player::{OutputFormat, Player, VirtualSink, transport_text};
use crossfade_core::Limits;
use crossfade_ffi::*;
use serde_json::Value;

struct Native {
    player: *mut CfPlayer,
    feed: *mut CfFeed,
    channels: usize,
}

impl Native {
    fn new(rate: u32, channels: u16) -> Self {
        let mut player = std::ptr::null_mut();
        assert_eq!(
            unsafe { cf_player_new(rate, channels, &mut player) },
            CF_OK,
            "{:?}",
            last_error()
        );
        let mut feed = std::ptr::null_mut();
        assert_eq!(unsafe { cf_player_take_feed(player, &mut feed) }, CF_OK);
        Self {
            player,
            feed,
            channels: usize::from(channels),
        }
    }

    fn load(&self, path: &str) {
        let path = c(path);
        assert_eq!(unsafe { cf_player_load(self.player, path.as_ptr()) }, CF_OK);
    }

    fn settle(&self) {
        assert_eq!(unsafe { cf_player_settle(self.player) }, CF_OK);
    }

    fn pull(&self, frames: usize) -> Vec<f32> {
        let mut out = vec![1.0f32; frames * self.channels];
        assert_eq!(
            unsafe { cf_feed_fill(self.feed, out.as_mut_ptr(), out.len()) },
            CF_OK
        );
        out
    }

    fn pull_planar(&self, frames: usize) -> Vec<Vec<f32>> {
        let mut planes = vec![vec![1.0f32; frames]; self.channels];
        let ptrs: Vec<*mut f32> = planes.iter_mut().map(|p| p.as_mut_ptr()).collect();
        assert_eq!(
            unsafe { cf_feed_fill_planar(self.feed, ptrs.as_ptr(), ptrs.len(), frames) },
            CF_OK
        );
        planes
    }

    fn state(&self) -> Value {
        json_out(|out| unsafe { cf_player_state(self.player, out) })
    }

    fn position(&self) -> u64 {
        let mut at = u64::MAX;
        assert_eq!(unsafe { cf_player_position(self.player, &mut at) }, CF_OK);
        at
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        assert_eq!(unsafe { cf_player_destroy(self.player) }, CF_OK);
        unsafe { cf_feed_destroy(self.feed) };
    }
}

/// The same journey on the app layer's own player and virtual sink.
fn through_the_app(path: &str, format: OutputFormat, seek: u64, blocks: usize) -> Vec<f32> {
    let (sink, output) = VirtualSink::new(format);
    let mut player = Player::new(
        Box::new(sink),
        Box::new(FsIo::new()),
        Limits::default(),
        || {},
    )
    .unwrap();
    let mut got = Vec::new();
    player.load(path);
    player.settle();
    player.play();
    player.settle();
    for _ in 0..blocks {
        got.extend(output.pull(1024));
        player.settle();
    }
    player.seek(seek);
    player.settle();
    for _ in 0..blocks {
        got.extend(output.pull(1024));
        player.settle();
    }
    got
}

fn through_c(path: &str, format: OutputFormat, seek: u64, blocks: usize) -> Vec<f32> {
    let native = Native::new(format.sample_rate, format.channels);
    let mut got = Vec::new();
    native.load(path);
    native.settle();
    assert_eq!(unsafe { cf_player_play(native.player) }, CF_OK);
    native.settle();
    for _ in 0..blocks {
        got.extend(native.pull(1024));
        native.settle();
    }
    assert_eq!(unsafe { cf_player_seek(native.player, seek) }, CF_OK);
    native.settle();
    assert_eq!(native.position(), seek);
    for _ in 0..blocks {
        got.extend(native.pull(1024));
        native.settle();
    }
    got
}

#[test]
fn the_pull_side_gives_the_virtual_sinks_samples_through_a_seek() {
    let dir = Dir::new("player-pull");
    let path = write_wav(&dir, "a.wav", 44_100, 2, 44_100 * 3);
    for format in [
        OutputFormat {
            sample_rate: 48_000,
            channels: 2,
        },
        OutputFormat {
            sample_rate: 44_100,
            channels: 1,
        },
    ] {
        let want = through_the_app(&path, format, 66_150, 30);
        let got = through_c(&path, format, 66_150, 30);
        assert_eq!(got.len(), want.len());
        assert!(got.iter().any(|s| *s != 0.0));
        assert!(got == want, "{format:?}");
    }
}

#[test]
fn planar_pulls_are_the_interleaved_samples_deinterleaved() {
    let dir = Dir::new("player-planar");
    let path = write_wav(&dir, "a.wav", 48_000, 2, 48_000);
    let a = Native::new(48_000, 2);
    let b = Native::new(48_000, 2);
    for n in [&a, &b] {
        n.load(&path);
        n.settle();
        assert_eq!(unsafe { cf_player_play(n.player) }, CF_OK);
        n.settle();
    }
    // Bigger than the feed's scratch, so it goes in several passes.
    for frames in [5000, 17, 3000] {
        let interleaved = a.pull(frames);
        let planes = b.pull_planar(frames);
        for (ch, plane) in planes.iter().enumerate() {
            let want: Vec<f32> = interleaved.iter().skip(ch).step_by(2).copied().collect();
            assert!(*plane == want, "channel {ch}, {frames} frames");
        }
        a.settle();
        b.settle();
    }
}

#[test]
fn the_state_follows_load_play_and_the_end() {
    let dir = Dir::new("player-state");
    let path = write_wav(&dir, "a.wav", 48_000, 2, 4_800);
    let n = Native::new(48_000, 2);
    assert_eq!(n.state()["state"], "empty");
    n.load(&path);
    n.settle();
    let s = n.state();
    assert_eq!(s["state"], "ready");
    assert_eq!(s["duration"], 4_800);
    assert_eq!(s["sample_rate"], 48_000);
    assert_eq!(s["transport"], transport_text(0, Some(4_800), 48_000));
    assert_eq!(s["output"]["sample_rate"], 48_000);
    assert_eq!(s["device_running"], false);
    assert_eq!(unsafe { cf_player_play(n.player) }, CF_OK);
    n.settle();
    assert_eq!(n.state()["state"], "playing");
    assert_eq!(n.state()["device_running"], true);
    n.pull(4_800);
    n.settle();
    n.pull(1);
    n.settle();
    let s = n.state();
    assert_eq!(s["state"], "ended", "{s}");
    assert_eq!(n.position(), 4_800);
    assert_eq!(unsafe { cf_player_stop(n.player) }, CF_OK);
    n.settle();
    assert_eq!(n.state()["state"], "ready");
    assert_eq!(n.position(), 0);
}

#[test]
fn toggle_pause_and_scrub() {
    let dir = Dir::new("player-scrub");
    let path = write_wav(&dir, "a.wav", 48_000, 1, 48_000);
    let n = Native::new(48_000, 1);
    n.load(&path);
    n.settle();
    assert_eq!(unsafe { cf_player_toggle(n.player) }, CF_OK);
    assert_eq!(n.state()["state"], "playing");
    assert_eq!(unsafe { cf_player_pause(n.player) }, CF_OK);
    assert_eq!(n.state()["state"], "paused");
    assert_eq!(unsafe { cf_player_scrub(n.player, 24_000) }, CF_OK);
    assert_eq!(n.state()["scrubbing"], true);
    assert_eq!(n.position(), 24_000);
    assert_eq!(unsafe { cf_player_end_scrub(n.player) }, CF_OK);
    n.settle();
    assert_eq!(n.state()["scrubbing"], false);
    assert_eq!(n.position(), 24_000);
}

#[test]
fn a_file_that_will_not_play_fails_as_a_state_and_the_wait_wakes() {
    let dir = Dir::new("player-bad");
    let junk = dir.path("junk.wav");
    std::fs::write(&junk, b"nope").unwrap();
    let n = Native::new(48_000, 2);
    let at = n.state()["revision"].as_u64().unwrap();
    n.load(&junk);
    let mut now = 0;
    assert_eq!(
        unsafe { cf_player_wait(n.player, at, 60_000, &mut now) },
        CF_OK
    );
    assert!(now > at);
    n.settle();
    let s = n.state();
    assert_eq!(s["state"], "failed");
    assert!(s["failure"].is_string());
}

#[test]
fn only_the_creating_thread_drives_the_player_but_any_thread_may_wait() {
    let n = Native::new(48_000, 2);
    let player = n.player as usize;
    let (play, wait, destroy) = std::thread::spawn(move || {
        let p = player as *mut CfPlayer;
        let play = unsafe { cf_player_play(p) };
        let message = last_error().unwrap_or_default();
        let mut now = 0;
        let wait = unsafe { cf_player_wait(p, u64::MAX, 0, &mut now) };
        let destroy = unsafe { cf_player_destroy(p) };
        ((play, message), wait, destroy)
    })
    .join()
    .unwrap();
    assert_eq!(play.0, CF_ERR_WRONG_THREAD);
    assert!(play.1.contains("thread"), "{}", play.1);
    assert_eq!(wait, CF_OK);
    assert_eq!(destroy, CF_ERR_WRONG_THREAD);
    // Still alive and usable here.
    assert_eq!(n.state()["state"], "empty");
}

#[test]
fn the_feed_is_taken_once_and_misuse_is_refused() {
    let n = Native::new(48_000, 2);
    let mut again = std::ptr::null_mut();
    assert_eq!(
        unsafe { cf_player_take_feed(n.player, &mut again) },
        CF_ERR_REFUSED
    );
    assert!(again.is_null());
    // A null feed is refused without touching the buffer; an empty buffer is fine.
    let mut buf = [0.5f32; 4];
    assert_eq!(
        unsafe { cf_feed_fill(std::ptr::null_mut(), buf.as_mut_ptr(), 4) },
        CF_ERR_NULL_ARGUMENT
    );
    assert_eq!(buf, [0.5; 4]);
    assert_eq!(
        unsafe { cf_feed_fill(n.feed, std::ptr::null_mut(), 0) },
        CF_OK
    );
    assert_eq!(
        unsafe { cf_feed_fill(n.feed, std::ptr::null_mut(), 4) },
        CF_ERR_NULL_ARGUMENT
    );
    // Nothing loaded: silence.
    assert_eq!(n.pull(3), vec![0.0; 6]);
    let planes: [*mut f32; 1] = [buf.as_mut_ptr()];
    assert_eq!(
        unsafe { cf_feed_fill_planar(n.feed, planes.as_ptr(), 1, 4) },
        CF_ERR_REFUSED
    );
    let mut channels = 0;
    assert_eq!(unsafe { cf_feed_channels(n.feed, &mut channels) }, CF_OK);
    assert_eq!(channels, 2);
    unsafe { cf_feed_destroy(std::ptr::null_mut()) };
    assert_eq!(unsafe { cf_player_destroy(std::ptr::null_mut()) }, CF_OK);
}

#[test]
fn a_device_with_no_channels_is_refused() {
    let mut player = std::ptr::null_mut();
    assert_eq!(
        unsafe { cf_player_new(48_000, 0, &mut player) },
        CF_ERR_REFUSED
    );
    assert!(player.is_null());
    assert!(last_error().unwrap().contains("0 channels"));
}

#[test]
fn the_feed_outlives_its_player_and_plays_silence() {
    let dir = Dir::new("player-outlive");
    let path = write_wav(&dir, "a.wav", 48_000, 2, 48_000);
    let mut player = std::ptr::null_mut();
    assert_eq!(unsafe { cf_player_new(48_000, 2, &mut player) }, CF_OK);
    let mut feed = std::ptr::null_mut();
    assert_eq!(unsafe { cf_player_take_feed(player, &mut feed) }, CF_OK);
    let p = c(&path);
    assert_eq!(unsafe { cf_player_load(player, p.as_ptr()) }, CF_OK);
    assert_eq!(unsafe { cf_player_play(player) }, CF_OK);
    assert_eq!(unsafe { cf_player_destroy(player) }, CF_OK);
    let mut out = [1.0f32; 8];
    assert_eq!(unsafe { cf_feed_fill(feed, out.as_mut_ptr(), 8) }, CF_OK);
    assert_eq!(out, [0.0; 8]);
    unsafe { cf_feed_destroy(feed) };
}
