/*
 * crossfade.h: the C interface to Crossfade's app layer, for the native mobile apps.
 *
 * Written by hand; crates/crossfade-ffi/tests/header.rs checks that it declares exactly the
 * functions the library exports and that its constants match. README.md beside this file
 * is the contract: the JSON shapes, the threading model and the memory rules.
 *
 * Conventions
 * - Strings in are UTF-8 and NUL-terminated, borrowed for the call only.
 * - Most functions return a status: CF_OK, or an error code whose message the same thread
 *   reads with cf_last_error_message. A successful call clears that message.
 * - Every string returned through a char ** out parameter, and every non-NULL result of
 *   cf_last_error_message, is the caller's: free it with cf_string_free, once.
 * - Every handle has one destroy function. Passing NULL is always safe: functions return
 *   CF_ERR_NULL_ARGUMENT and destroy functions do nothing. Using a handle after destroying
 *   it is the caller's bug and cannot be detected.
 * - Out parameters are written only on CF_OK.
 * - No function calls back into the caller. Work runs on the library's own threads; the
 *   caller learns of changes by reading state (cheap) or by waiting on a revision.
 * - Blocking: only the *_wait functions (up to their timeout), cf_player_settle, and
 *   cf_queue_destroy (briefly, while the running job stops) block. Everything else returns
 *   promptly and is safe on a UI thread.
 * - The library is built with panic=unwind: an internal error becomes CF_ERR_PANIC.
 */

#ifndef CROSSFADE_H
#define CROSSFADE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* The interface version cf_abi_version returns; it changes only when an existing function
 * changes meaning or shape. New functions are added without changing it. */
#define CF_ABI_VERSION 1

/* Status codes. */
#define CF_OK 0
/* A pointer the call needs was NULL. */
#define CF_ERR_NULL_ARGUMENT 1
/* A string was not UTF-8. */
#define CF_ERR_INVALID_UTF8 2
/* JSON that does not parse, or a key or value the documented shape does not have. */
#define CF_ERR_INVALID_JSON 3
/* The model refused; the message says why in the app's own words, fit to show. */
#define CF_ERR_REFUSED 4
/* No job with that id. */
#define CF_ERR_NOT_FOUND 5
/* A player was used from a thread other than the one that made it. */
#define CF_ERR_WRONG_THREAD 6
/* Something failed that is not the caller's doing. */
#define CF_ERR_FAILED 7
/* An internal error was caught at the boundary; nothing the call did can be relied on. */
#define CF_ERR_PANIC 8

/* Opaque handles. */
typedef struct CfSheet CfSheet;
typedef struct CfQueue CfQueue;
typedef struct CfAnalysis CfAnalysis;
typedef struct CfPlayer CfPlayer;
typedef struct CfFeed CfFeed;

/* ---- Versions, strings, errors ------------------------------------------------------ */

/* The interface version, CF_ABI_VERSION for this header. Compare at start-up. Any thread. */
uint32_t cf_abi_version(void);

/* The library's version, such as "0.0.0". A static string: never free it. Any thread. */
const char *cf_version(void);

/* "About Crossfade" as JSON: {"lines": [...], "version": "...", "abi_version": 1}.
 * Writes a new string to *out (free with cf_string_free). Any thread. */
int32_t cf_about(char **out);

/* Frees a string the library returned. NULL does nothing. Any thread. */
void cf_string_free(char *s);

/* The message of this thread's last call if it failed, else NULL. A new string: free it
 * with cf_string_free. Any thread; each thread has its own. */
char *cf_last_error_message(void);

/* ---- Files -------------------------------------------------------------------------- */

/* Describes the file at path (probes its headers, does not decode) as JSON: the inspector's
 * sections, the files list's summary, or why it cannot be read. An unreadable file is
 * CF_OK with "readable": false and its "problem". Reads the file: call it off the UI
 * thread for files that may be slow to open. Any thread. */
int32_t cf_inspect(const char *path, char **out);

/* ---- Convert sheet ------------------------------------------------------------------ */

/* Opens a convert sheet for the file at path into *out. A file the inspector cannot read
 * has no sheet: CF_ERR_REFUSED with the inspector's problem. Reads the file's headers.
 * Destroy with cf_sheet_destroy. Any thread; calls on one sheet are serialised. */
int32_t cf_sheet_new(const char *path, CfSheet **out);

/* Applies changes, a JSON object (its keys in the documented order) or a list of objects
 * (in turn), through the sheet's own setters. All or none: CF_ERR_INVALID_JSON for an
 * unknown key or a value of the wrong kind, and the sheet is unchanged. Any thread. */
int32_t cf_sheet_apply(const CfSheet *sheet, const char *changes);

/* The sheet's whole state as JSON into *out: every option with what is offered, visible
 * and enabled, the problems, the notes, the command line, "can_queue" and the request.
 * Checks whether the output path exists. Any thread. */
int32_t cf_sheet_state(const CfSheet *sheet, char **out);

/* Destroys a sheet. NULL does nothing. Not while another thread is using it. */
void cf_sheet_destroy(CfSheet *sheet);

/* ---- Queue -------------------------------------------------------------------------- */

/* Makes an empty conversion queue into *out. Jobs run one at a time on a library thread,
 * writing through a temporary file renamed into place on success. Any thread. */
int32_t cf_queue_new(CfQueue **out);

/* Queues the request the sheet makes and writes its id to *id. A sheet with problems
 * queues nothing: CF_ERR_REFUSED with its problems' messages. The sheet stays the
 * caller's. Any thread. */
int32_t cf_queue_add(const CfQueue *queue, const CfSheet *sheet, uint64_t *id);

/* The queue's state as JSON into *out, with its "revision". Cheap; any thread. */
int32_t cf_queue_state(const CfQueue *queue, char **out);

/* Blocks until the queue's revision differs from since, or timeout_ms passes (0 returns at
 * once), and writes the revision then to *revision. Park a background thread or task on
 * it; never a UI thread. Any thread. */
int32_t cf_queue_wait(const CfQueue *queue, uint64_t since, uint32_t timeout_ms,
                      uint64_t *revision);

/* Cancels a job: a waiting one at once, a running one as soon as it next checks, which
 * removes its partial output. CF_ERR_NOT_FOUND for an unknown id. Any thread. */
int32_t cf_queue_cancel(const CfQueue *queue, uint64_t id);

/* Queues a failed or cancelled job again in its row's place and writes the new id to
 * *new_id; the old id is gone. CF_ERR_REFUSED for any other state. Any thread. */
int32_t cf_queue_retry(const CfQueue *queue, uint64_t id, uint64_t *new_id);

/* Removes a job that is not running; CF_ERR_REFUSED for a running one. Any thread. */
int32_t cf_queue_remove(const CfQueue *queue, uint64_t id);

/* Removes every finished job. Any thread. */
int32_t cf_queue_clear_done(const CfQueue *queue);

/* Cancels every job, blocks until the running one has stopped and removed its partial
 * output, and frees the queue. NULL does nothing. Not while another thread is using it. */
void cf_queue_destroy(CfQueue *queue);

/* ---- Analysis ----------------------------------------------------------------------- */

/* Starts analysing the file at path (waveform and loudness) on a library thread and
 * writes the handle to *out. An unreadable file still gives a handle, whose state ends
 * "failed". Any thread. */
int32_t cf_analysis_new(const char *path, CfAnalysis **out);

/* The analysis's state as JSON into *out: progress, loudness, the readout line, the lane
 * count, with its "revision". Cheap; any thread. */
int32_t cf_analysis_state(const CfAnalysis *analysis, char **out);

/* Blocks until the revision differs from since, or timeout_ms passes, and writes it to
 * *revision. Never on a UI thread with a timeout above 0. Any thread. */
int32_t cf_analysis_wait(const CfAnalysis *analysis, uint64_t since, uint32_t timeout_ms,
                         uint64_t *revision);

/* Writes `columns` waveform columns of lane `lane`, each frames_per_column frames wide from
 * start_frame, as a minimum into mins[i] and a maximum into maxs[i] (linear, -1 to 1), and
 * the number of columns with data to *written; the rest, past what is analysed, are NaN.
 * mins and maxs are the caller's, `columns` floats each (NULL allowed when columns is 0).
 * CF_ERR_REFUSED for a lane the file does not have. Brief lock; any thread. */
int32_t cf_analysis_columns(const CfAnalysis *analysis, uint32_t lane, uint64_t start_frame,
                            uint64_t frames_per_column, float *mins, float *maxs,
                            size_t columns, size_t *written);

/* Stops the analysis; its state ends "cancelled" (or "done" if it finished first). */
int32_t cf_analysis_cancel(const CfAnalysis *analysis);

/* Cancels and frees the analysis without waiting for its thread. NULL does nothing. */
void cf_analysis_destroy(CfAnalysis *analysis);

/* ---- Player ------------------------------------------------------------------------- */

/* Makes a preview player for an output device running at sample_rate with `channels`
 * interleaved channels and writes it to *out. The native app owns the device and pulls
 * samples through the player's feed (cf_player_take_feed). Call every cf_player_ function
 * except cf_player_wait from this same thread (the main thread is the natural choice);
 * another thread gets CF_ERR_WRONG_THREAD. CF_ERR_REFUSED for a rate or count of 0. When
 * the device's format changes, destroy the player and make a new one. */
int32_t cf_player_new(uint32_t sample_rate, uint16_t channels, CfPlayer **out);

/* Takes the player's feed, once, into *out; CF_ERR_REFUSED the second time. Hand it to the
 * audio callback. Destroy it with cf_feed_destroy once the callback can no longer run. */
int32_t cf_player_take_feed(const CfPlayer *player, CfFeed **out);

/* Loads the file at path, stopped at its start. Returns at once; the state goes "loading"
 * then "ready" or "failed". */
int32_t cf_player_load(const CfPlayer *player, const char *path);

/* Plays (from the start again once ended). */
int32_t cf_player_play(const CfPlayer *player);

/* Pauses. */
int32_t cf_player_pause(const CfPlayer *player);

/* Plays if paused or stopped, pauses if playing. */
int32_t cf_player_toggle(const CfPlayer *player);

/* Stops and goes back to the start. */
int32_t cf_player_stop(const CfPlayer *player);

/* Jumps to source frame `frame` (held to the duration); playing carries on from there. */
int32_t cf_player_seek(const CfPlayer *player, uint64_t frame);

/* Follows a drag along the waveform: output goes silent and the position is `frame`
 * until cf_player_end_scrub. */
int32_t cf_player_scrub(const CfPlayer *player, uint64_t frame);

/* Ends a drag: seeks to where it was let go and carries on as before it began. */
int32_t cf_player_end_scrub(const CfPlayer *player);

/* Blocks until the decoder has carried out every command and filled its buffer as far as
 * it can. For tests and screenshots; an app does not need it. */
int32_t cf_player_settle(const CfPlayer *player);

/* The position in source frames into *frames: what the device has played. Cheap enough to
 * call every display frame. */
int32_t cf_player_position(const CfPlayer *player, uint64_t *frames);

/* The player's state as JSON into *out, with its "revision". */
int32_t cf_player_state(const CfPlayer *player, char **out);

/* Blocks until the revision differs from since (a load finished, playback reached the
 * end, a command was given) or timeout_ms passes, and writes it to *revision. The one
 * player function any thread may call; never on a UI thread with a timeout above 0. */
int32_t cf_player_wait(const CfPlayer *player, uint64_t since, uint32_t timeout_ms,
                       uint64_t *revision);

/* Stops the decoder thread and frees the player, from the thread that made it
 * (CF_ERR_WRONG_THREAD otherwise, and nothing is freed). A feed taken from it stays valid
 * and plays silence. NULL does nothing and returns CF_OK. */
int32_t cf_player_destroy(CfPlayer *player);

/* ---- Feed: the audio callback's side ------------------------------------------------ */

/* Fills `samples` interleaved floats at out with the next audio, or silence. REAL-TIME
 * SAFE: never blocks, locks, allocates or frees, and sets no error message. Call it from
 * the device's render callback, from one thread at a time. A trailing partial frame is
 * silence. Returns CF_OK, or CF_ERR_NULL_ARGUMENT for a NULL feed (or a NULL out with
 * samples above 0), touching nothing. */
int32_t cf_feed_fill(CfFeed *feed, float *out, size_t samples);

/* Fills `frames` frames into one buffer per channel: planes points at `channels` buffers of
 * `frames` floats each (the layout AVAudioSourceNode's default format gives). REAL-TIME
 * SAFE, as cf_feed_fill. CF_ERR_REFUSED when channels is not the feed's count. */
int32_t cf_feed_fill_planar(CfFeed *feed, float *const *planes, size_t channels,
                            size_t frames);

/* The feed's channel count into *channels: the player's device count. */
int32_t cf_feed_channels(const CfFeed *feed, size_t *channels);

/* Frees a feed. Never from the audio callback: stop the device first. NULL does nothing. */
void cf_feed_destroy(CfFeed *feed);

#ifdef __cplusplus
}
#endif

#endif /* CROSSFADE_H */
