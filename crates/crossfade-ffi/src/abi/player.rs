//! The player's functions, and the feed's, which an audio callback calls.

use super::{destroy, handle, handle_out, str_arg, string_out, write_out};
use crate::error::{CF_ERR_NULL_ARGUMENT, CF_ERR_PANIC, CF_ERR_REFUSED, CF_OK, Failure, guard};
use crate::player::{NativePlayer, PullFeed};
use std::ffi::c_char;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

/// The preview player, driven from the thread that made it.
pub type CfPlayer = NativePlayer;
/// The player's pull side, for the audio callback.
pub type CfFeed = PullFeed;

/// Makes a player for an output device at `sample_rate` with `channels` channels.
///
/// # Safety
/// `out` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_player_new(
    sample_rate: u32,
    channels: u16,
    out: *mut *mut CfPlayer,
) -> i32 {
    guard(|| {
        if out.is_null() {
            return Err(Failure::null("the out pointer"));
        }
        let player = NativePlayer::new(sample_rate, channels)?;
        // SAFETY: the caller's promise about `out`.
        unsafe { handle_out(out, player) }
    })
}

/// Takes the player's feed, once.
///
/// # Safety
/// `player` is null or a live player; `out` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_player_take_feed(
    player: *const CfPlayer,
    out: *mut *mut CfFeed,
) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `player`.
        let player = unsafe { handle(player, "the player") }?;
        if out.is_null() {
            return Err(Failure::null("the out pointer"));
        }
        let feed = player.take_feed()?;
        // SAFETY: the caller's promise about `out`.
        unsafe { handle_out(out, feed) }
    })
}

/// Runs one of the player's commands.
///
/// # Safety
/// `player` is null or a live player.
unsafe fn command(
    player: *const CfPlayer,
    f: impl FnOnce(&NativePlayer) -> Result<(), Failure>,
) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `player`.
        let player = unsafe { handle(player, "the player") }?;
        f(player)
    })
}

/// Loads the file at `path`, stopped at its start; the state says when it is ready.
///
/// # Safety
/// `player` is null or a live player; `path` is null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_player_load(player: *const CfPlayer, path: *const c_char) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `player`.
        let player = unsafe { handle(player, "the player") }?;
        // SAFETY: the caller's promise about `path`.
        let path = unsafe { str_arg(path, "the path") }?;
        player.load(path)
    })
}

/// Plays (from the start again once ended).
///
/// # Safety
/// `player` is null or a live player.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_player_play(player: *const CfPlayer) -> i32 {
    // SAFETY: the caller's promise about `player`.
    unsafe { command(player, NativePlayer::play) }
}

/// Pauses.
///
/// # Safety
/// `player` is null or a live player.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_player_pause(player: *const CfPlayer) -> i32 {
    // SAFETY: the caller's promise about `player`.
    unsafe { command(player, NativePlayer::pause) }
}

/// Plays if paused, pauses if playing.
///
/// # Safety
/// `player` is null or a live player.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_player_toggle(player: *const CfPlayer) -> i32 {
    // SAFETY: the caller's promise about `player`.
    unsafe { command(player, NativePlayer::toggle) }
}

/// Stops and goes back to the start.
///
/// # Safety
/// `player` is null or a live player.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_player_stop(player: *const CfPlayer) -> i32 {
    // SAFETY: the caller's promise about `player`.
    unsafe { command(player, NativePlayer::stop) }
}

/// Jumps to source frame `frame`.
///
/// # Safety
/// `player` is null or a live player.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_player_seek(player: *const CfPlayer, frame: u64) -> i32 {
    // SAFETY: the caller's promise about `player`.
    unsafe { command(player, |p| p.seek(frame)) }
}

/// Follows a drag: silent, the position is `frame`, until `cf_player_end_scrub`.
///
/// # Safety
/// `player` is null or a live player.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_player_scrub(player: *const CfPlayer, frame: u64) -> i32 {
    // SAFETY: the caller's promise about `player`.
    unsafe { command(player, |p| p.scrub(frame)) }
}

/// Ends a drag: seeks where it was let go and carries on as before it began.
///
/// # Safety
/// `player` is null or a live player.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_player_end_scrub(player: *const CfPlayer) -> i32 {
    // SAFETY: the caller's promise about `player`.
    unsafe { command(player, NativePlayer::end_scrub) }
}

/// Blocks until the decoder has done every command and filled what it can. For tests.
///
/// # Safety
/// `player` is null or a live player.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_player_settle(player: *const CfPlayer) -> i32 {
    // SAFETY: the caller's promise about `player`.
    unsafe { command(player, NativePlayer::settle) }
}

/// The position in source frames, cheaply.
///
/// # Safety
/// `player` is null or a live player; `frames` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_player_position(player: *const CfPlayer, frames: *mut u64) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `player`.
        let player = unsafe { handle(player, "the player") }?;
        if frames.is_null() {
            return Err(Failure::null("the frames pointer"));
        }
        let at = player.position()?;
        // SAFETY: the caller's promise about `frames`.
        unsafe { write_out(frames, at, "the frames pointer") }
    })
}

/// The player's state as JSON.
///
/// # Safety
/// `player` is null or a live player; `out` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_player_state(player: *const CfPlayer, out: *mut *mut c_char) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `player`.
        let player = unsafe { handle(player, "the player") }?;
        if out.is_null() {
            return Err(Failure::null("the out pointer"));
        }
        let json = player.state_json()?;
        // SAFETY: the caller's promise about `out`.
        unsafe { string_out(out, json) }
    })
}

/// Blocks until the revision differs from `since` or `timeout_ms` passes; writes it. Any
/// thread may call it.
///
/// # Safety
/// `player` is null or a live player; `revision` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_player_wait(
    player: *const CfPlayer,
    since: u64,
    timeout_ms: u32,
    revision: *mut u64,
) -> i32 {
    guard(|| {
        if player.is_null() {
            return Err(Failure::null("the player"));
        }
        if revision.is_null() {
            return Err(Failure::null("the revision pointer"));
        }
        // SAFETY: a live player (the caller's promise). Only the counter field is borrowed,
        // an `Arc<Signal>`, which is `Sync`; no reference to the player itself, whose other
        // parts stay on the thread that made it, is made here.
        let signal = unsafe { &(*player).signal };
        let now = NativePlayer::wait(signal, since, Duration::from_millis(u64::from(timeout_ms)));
        // SAFETY: the caller's promise about `revision`.
        unsafe { write_out(revision, now, "the revision pointer") }
    })
}

/// Stops the decoder and frees the player, from the thread that made it, once no other thread
/// is waiting on it. Null does nothing. A feed taken from it plays silence afterwards.
///
/// # Safety
/// `player` is null or a player from `cf_player_new` not yet destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_player_destroy(player: *mut CfPlayer) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `player`.
        if let Some(p) = unsafe { player.as_ref() } {
            p.check_thread()?;
        }
        // SAFETY: as above; checked to be on the owner thread.
        unsafe { destroy(player) };
        Ok(())
    })
}

/// Runs a fill without the guard: no message is kept, so nothing is allocated.
fn realtime(f: impl FnOnce() -> i32) -> i32 {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(CF_ERR_PANIC)
}

/// Fills `samples` interleaved floats with the next audio, or silence. Real-time safe.
///
/// # Safety
/// `feed` is null or a live feed used by no other thread during the call; `out` is null or
/// valid for `samples` writes (it may be null when `samples` is 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_feed_fill(feed: *mut CfFeed, out: *mut f32, samples: usize) -> i32 {
    realtime(|| {
        // SAFETY: the caller's promise: null or live, and this thread alone uses it now.
        let Some(feed) = (unsafe { feed.as_mut() }) else {
            return CF_ERR_NULL_ARGUMENT;
        };
        if samples == 0 {
            return CF_OK;
        }
        if out.is_null() {
            return CF_ERR_NULL_ARGUMENT;
        }
        // SAFETY: not null, and the caller promises room for `samples` floats.
        let out = unsafe { std::slice::from_raw_parts_mut(out, samples) };
        feed.fill(out);
        CF_OK
    })
}

/// Fills `frames` frames into one buffer per channel. Real-time safe.
///
/// # Safety
/// `feed` is null or a live feed used by no other thread during the call; `planes` is null
/// or points at `channels` pointers, each valid for `frames` writes, none overlapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_feed_fill_planar(
    feed: *mut CfFeed,
    planes: *const *mut f32,
    channels: usize,
    frames: usize,
) -> i32 {
    realtime(|| {
        // SAFETY: the caller's promise: null or live, and this thread alone uses it now.
        let Some(feed) = (unsafe { feed.as_mut() }) else {
            return CF_ERR_NULL_ARGUMENT;
        };
        if channels != feed.channels() {
            return CF_ERR_REFUSED;
        }
        if frames == 0 {
            return CF_OK;
        }
        if planes.is_null() {
            return CF_ERR_NULL_ARGUMENT;
        }
        // SAFETY: not null, and the caller promises `channels` pointers there.
        let planes = unsafe { std::slice::from_raw_parts(planes, channels) };
        if planes.iter().any(|p| p.is_null()) {
            return CF_ERR_NULL_ARGUMENT;
        }
        feed.fill_planar(frames, |first, chunk| {
            let n = chunk.len() / channels;
            for (ch, plane) in planes.iter().enumerate() {
                // SAFETY: each plane has room for `frames` floats (the caller's promise) and
                // `first + n <= frames`; the planes do not overlap.
                let dst = unsafe { std::slice::from_raw_parts_mut(plane.add(first), n) };
                for (i, d) in dst.iter_mut().enumerate() {
                    *d = chunk[i * channels + ch];
                }
            }
        });
        CF_OK
    })
}

/// The feed's channel count.
///
/// # Safety
/// `feed` is null or a live feed; `channels` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_feed_channels(feed: *const CfFeed, channels: *mut usize) -> i32 {
    guard(|| {
        // SAFETY: the caller's promise about `feed`.
        let feed = unsafe { handle(feed, "the feed") }?;
        // SAFETY: the caller's promise about `channels`.
        unsafe { write_out(channels, feed.channels(), "the channels pointer") }
    })
}

/// Frees a feed. Not from the audio callback, and only once the callback can no longer run.
/// Null does nothing.
///
/// # Safety
/// `feed` is null or a feed from `cf_player_take_feed` not yet destroyed, in use on no other
/// thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cf_feed_destroy(feed: *mut CfFeed) {
    guard(|| {
        // SAFETY: the caller's promise about `feed`.
        unsafe { destroy(feed) };
        Ok(())
    });
}
