# crossfade-ffi

The C interface the native mobile apps (SwiftUI first, Kotlin later) use to drive Crossfade's
app layer. It exposes the `crossfade-app` models, not a copy of them: the inspector, the convert
sheet, the queue with its runner, the background analysis and the preview player. A phone UI
built on it offers exactly what the desktop sheet offers, so it cannot offer a conversion the
command line would refuse (the parity tests and a proptest check this through the C functions).

- `include/crossfade.h` is the header, written by hand (no binding generator). `tests/header.rs`
  checks that it declares exactly the functions the library exports (`EXPORTS`, itself checked
  against the sources) and that its constants match.
- Built as `staticlib` (an iOS XCFramework: `scripts/build-xcframework.sh`, not yet run, needs
  Xcode), `cdylib` (Android, and loading from other languages: `scripts/ffi-journey.py` drives
  it from Python's `ctypes`) and `rlib` (the Rust tests).
- `unsafe` lives only in `src/abi/`, at the edge: reading a caller's pointer, taking or giving
  back a box. Each block says why it is sound. The logic is in safe modules.
- No new dependency: `crossfade-app` beneath, `serde` and `serde_json` for the documents.
- AAC stays off: the `aac` feature passes through to the app layer and is off by default.

## Conventions

| | |
|---|---|
| Strings in | UTF-8, NUL-terminated, borrowed for the call. Invalid UTF-8 is `CF_ERR_INVALID_UTF8`. |
| Status | Most functions return `int32_t`: `CF_OK` (0) or an error code. The message, in the app's words and fit to show, comes from `cf_last_error_message()` on the same thread. A successful call clears it. |
| Strings out | Through `char **out`, written only on `CF_OK`. Every such string, and every non-null `cf_last_error_message()`, is the caller's: free it once with `cf_string_free`. `cf_version()` is static: never free it. |
| Handles | Opaque pointers. One `*_new` (or `cf_player_take_feed`), one `*_destroy`. Null is always safe: functions return `CF_ERR_NULL_ARGUMENT`, destroy functions do nothing. Use after destroy is the caller's bug and cannot be detected. |
| Numbers out | Through pointers (`uint64_t *id`, `size_t *written`), written only on `CF_OK`. |
| Bulk numbers | Caller's buffers: waveform columns (`float` mins and maxs), audio (`float` interleaved, or one buffer per channel). A length of 0 takes null buffers. |
| Panics | Caught at every entry point: `CF_ERR_PANIC`, never an unwind into C. Build with `panic = "unwind"` (the default). |
| Versions | `cf_abi_version()` is `CF_ABI_VERSION` (1). It changes only when an existing function changes meaning or shape; new functions and new JSON keys do not change it, so decode JSON leniently (ignore unknown keys). |

Status codes: `CF_OK` 0, `CF_ERR_NULL_ARGUMENT` 1, `CF_ERR_INVALID_UTF8` 2,
`CF_ERR_INVALID_JSON` 3 (malformed, or a key or value the shape does not have),
`CF_ERR_REFUSED` 4 (the model said no: the message says why), `CF_ERR_NOT_FOUND` 5 (job id),
`CF_ERR_WRONG_THREAD` 6 (player), `CF_ERR_FAILED` 7, `CF_ERR_PANIC` 8.

## Threading

No function calls back into the app. Work runs on the library's own threads, as on the
desktop: the queue's `ThreadRunner` (one job at a time), one `Analyzer` thread per analysis,
the player's decode thread. The app learns of changes by asking:

- **State** (`cf_*_state`): a JSON document with a `revision`. Cheap; safe on a UI thread.
- **Wait** (`cf_queue_wait`, `cf_analysis_wait`, `cf_player_wait`): blocks until the revision
  differs from the one given, or the timeout passes (0 returns at once), and returns the
  revision. Park a background thread or a detached task on it, then read the state.

What blocks: only the three waits (up to their timeout), `cf_player_settle` (tests),
`cf_queue_destroy` (briefly, while the running job notices the cancel and removes its partial
file), and the file reads in `cf_inspect` and `cf_sheet_new` (headers only; a slow volume makes
them slow). Everything else returns at once.

Which thread:

- Sheets, queues and analyses may be used from any thread; calls on one handle are serialised.
  Destroy a handle when no other thread is using it.
- A player is driven only from the thread that made it (its sink is not `Send` in the app
  layer); other threads get `CF_ERR_WRONG_THREAD`. `cf_player_wait` is the exception: any
  thread may wait. Use the main thread (a `@MainActor` class in Swift).
- A feed is called from the audio thread, one call at a time. `cf_feed_fill` and
  `cf_feed_fill_planar` are real-time safe: no lock, no allocation or free, no blocking, no error
  message (codes only). That is by construction (the app layer's `Feed::fill` touches only
  atomics and `rtrb` rings; the planar scratch buffer is allocated when the feed is taken; the
  shim skips the error message and only catches unwinding) and is tested: `tests/realtime.rs`
  counts every allocator call on the calling thread across playing, a seek, an underrun, the end,
  pause and a destroyed player, and finds none.

How often to ask: wait for the queue and an analysis on a background task (a timeout of a
second or so lets it notice that it should stop) and re-read state when the revision moves;
for the player's playhead read `cf_player_position` once per display frame while playing
(it is a few atomic loads) and `cf_player_state` when the revision moves (a load finished,
the end was reached, a command was given).

## Memory and files

- The library never sees a URL. It takes plain paths the native side has already made
  readable (inputs) or writable (the output's folder: the output is written to a hidden
  temporary file beside it, then renamed into place on success, through the app layer's `FsIo`).
- On iOS, call `startAccessingSecurityScopedResource()` on the input's URL before
  `cf_inspect`, `cf_sheet_new`, `cf_analysis_new` and `cf_player_load`, and keep it until the
  work that reads the file ends: the queue opens the input when the job starts (not when it is
  queued), an analysis reads until its state is no longer `running`, a player reads for as long
  as the file is loaded. The output's folder needs the same, for the whole job, since the
  temporary file goes there; or write into the app's container and export the result.
- A cancelled or failed job, and a destroyed queue, leave no partial file.

## The documents

Every key below is always present (`null` where there is no value). Snapshots in
`tests/snapshots/` pin the full documents; the examples here are trimmed.

### `cf_about`

```json
{"lines": ["Crossfade 0.0.0", "Converts, inspects and normalises audio files, in pure Rust.",
           "MIT or Apache-2.0; the Opus codec is BSD-3-Clause."],
 "version": "0.0.0", "abi_version": 1}
```

### `cf_inspect` (what a file is)

The inspector's sections as the desktop shows them, each row with the `probe --json` key it
shows (`field`), a stable `id`, a `label`, the friendly `value` and the probe's `raw` value;
`summary` is the files list's line. An unreadable file is not an error: `readable` is false and
`problem` says why.

```json
{"path": "/…/tagged.flac", "name": "tagged.flac", "readable": true, "problem": null,
 "summary": "FLAC · 44.1 kHz · stereo · 16-bit · 0:01.000",
 "sections": [{"title": "File", "rows": [
   {"field": "format", "id": "format", "label": "Format", "value": "FLAC", "raw": "flac"}, …]},
   {"title": "Stream 0", "rows": […]}, {"title": "Tags", "rows": […]}]}
```

### `cf_sheet_state` (the convert sheet)

One row per control: `value` (what the sheet holds), `label` (what it reads), `options` (what
is offered, each with its value or id and label), `visible` and `enabled` where the desktop hides
or disables the row. Ids: containers `wav flac aiff oga m4a opus` (the extension written);
formats `same u8 s16 s24 s32 f32 f64`; level modes `off gain normalize`; problem kinds
`no_output output_is_input output_exists extension source adjust`, with `row`
(`level trim fade`) for `adjust`.

```json
{"input": "/…/in.wav", "output": "/…/in.opus", "overwrite": false,
 "preset": {"value": "opus", "options": [{"id": "same", "label": "Same as source"}, …]},
 "container": {"value": "opus", "label": "Opus",
   "options": [{"id": "wav", "label": "WAV", "extension": "wav"}, …]},
 "format": {"value": "same", "label": "Same as source", "visible": false, "enabled": true,
   "hint": null, "options": [{"id": "same", "label": "Same as source"}]},
 "compression_level": {"value": null, "visible": false, "enabled": false,
   "options": [null, 0, 1, 2, 3, 4, 5, 6, 7, 8]},
 "bitrate": {"value": 96000, "label": "96 kb/s", "visible": true, "enabled": true,
   "options": [{"value": null, "label": "Default (128 kb/s)"}, {"value": 32000, "label": "32 kb/s"}, …]},
 "copy": {"value": false, "available": false},
 "sample_rate": {"value": 48000, "label": "48 kHz", "enabled": true,
   "options": [{"value": 48000, "label": "48 kHz"}]},
 "channels": {"value": null, "label": "Same as source", "enabled": true,
   "options": [{"value": null, "label": "Same as source"}, {"value": "mono", "label": "Mono"}]},
 "adjust": {"open": true, "enabled": true, "summary": "Normalize −16 LUFS · Fade out 0.5 s",
   "level_mode": "normalize", "level_value": "-16", "trim_start": "", "trim_end": "",
   "fade_in": "", "fade_out": "0.5", "problems": {"level": null, "trim": null, "fade": null}},
 "processing_note": "48 kHz · stereo · 0:01.000",
 "notes": ["Opus is always 48 kHz: resampling from 44100 Hz"],
 "problems": [],
 "command_line": "crossfade convert /…/in.wav -o /…/in.opus --bitrate 96000 --filter 'fade out=0.5s' --filter 'normalize -16LUFS'",
 "can_queue": true,
 "request": {"job": {"version": 1, "inputs": [{"path": "/…/in.wav"}],
   "outputs": [{"path": "/…/in.opus", "audio": {"bitrate": 96000, "filters": […]}}]},
   "overwrite": false}}
```

`notes` are the plan's assumptions in the sheet's words (the ones the CLI prints).
`processing_note` is the line under Adjust: what will be written, or why the controls are off.
`command_line` is shown while only the path is in the way (as on the desktop); `null` while the
choices themselves have a problem. `request` is the engine's own `Job` (the job file's shape).

**Changes** (`cf_sheet_apply`) are a JSON object, or a list of objects applied in turn. Each key
calls the sheet's own setter; one object's keys are applied in this order:

| Key | Value |
|---|---|
| `preset` | a preset id from `preset.options` |
| `container` | a container id |
| `output` | a path; its extension picks the container when it names one |
| `copy` | `true`/`false` (ignored while `copy.available` is false) |
| `format` | `"same"` or a format id |
| `compression_level` | `null` or 0 to 8 |
| `bitrate` | `null` (the default) or bits per second |
| `sample_rate` | `null` (the source's) or Hz |
| `channels` | `null`, a layout name (`mono`, `stereo`, `5.1`, …) or a count as text |
| `overwrite` | `true`/`false` |
| `adjust_open` | `true`/`false` |
| `level_mode` | `"off"`, `"gain"`, `"normalize"` (normalize fills in −16 when empty) |
| `level_value`, `trim_start`, `trim_end`, `fade_in`, `fade_out` | the text as typed (`-3`, `1:30`, `500ms`) |

An unknown key, a value of the wrong kind or an unknown id is `CF_ERR_INVALID_JSON` and nothing is
applied. A value of the right kind the model does not take (a level of 12) is left to the model,
as on the desktop; the state shows what it holds. Example:
`[{"container": "flac"}, {"compression_level": 8}, {"level_mode": "gain"}, {"level_value": "-3"}]`.

### `cf_queue_state`

```json
{"revision": 12, "idle": false,
 "summary": {"pending": 1, "running": 1, "finished": 0, "failed": 0, "cancelled": 0},
 "quit_warning": "2 conversions are still running or waiting. Quitting will cancel them.",
 "jobs": [{"id": 0, "input": "/…/in.wav", "output": "/…/in.flac",
   "command_line": "crossfade convert …", "state": "running",
   "frames_done": 52000, "frames_total": 96000, "fraction": 0.54, "cancelling": false,
   "frames": null, "message": null, "measuring": false, "normalization": null, "clipped": 0}]}
```

`state` is `pending running finished failed cancelled`. `frames_done`/`frames_total` while
running (`frames_total` may be null: then `fraction` is null), `frames` once finished (what the
written file holds), `message` once failed (the engine's words). `normalization`, after a
normalising job's measuring pass: `{"measured_lufs", "target_lufs", "reached_lufs", "gain_db",
"limited_by_ceiling", "ceiling_dbtp"}`. `clipped` counts saturated samples of a finished job.

### `cf_analysis_state`

```json
{"revision": 41, "state": "done", "failure": null, "frames_done": 192000, "frames": 192000,
 "sample_rate": 48000, "channels": 2, "lanes": 2,
 "loudness": {"integrated": -6.70, "range": 0.0, "true_peak": 0.50, "sample_peak": 0.5},
 "readout": {"text": "−6.7 LUFS · 0.0 LU range · −6.0 dBTP", "warn": false,
   "segments": [{"text": "−6.7 LUFS", "warn": false}, {"text": " · ", "warn": false}, …]}}
```

`state` is `running done failed cancelled`. Loudness is LUFS and LU; the peaks are linear
(`null` until measurable). `readout` is the strip's line, in pieces so a figure can take the
warning colour. `lanes` is the strip's lane plan: one per channel for mono and stereo, one
combined lane otherwise. `cf_analysis_columns(lane, start_frame, frames_per_column, …)` fills a
min and a max per column; a whole view is `frames_per_column = ceil(frames / width)` from 0.

### `cf_player_state`

```json
{"revision": 3, "state": "ready", "failure": null, "position": 0, "duration": 88200,
 "sample_rate": 44100, "channels": 2, "transport": "0:00.0 / 0:02.0", "scrubbing": false,
 "device_running": false, "underruns": 0, "pulled": 0,
 "output": {"sample_rate": 48000, "channels": 2}}
```

`state` is `empty loading ready playing paused ended failed`. `position` and `duration` are in
the source's frames; `transport` is the desktop's readout. `device_running` says whether the
player wants the device pulling (playing); the feed gives silence otherwise, so leaving the
device running is also fine.

## Driving each screen

- **Files list and inspector.** `cf_inspect(path)` per file the user adds (off the main thread
  for slow volumes); show `summary` under the name, `sections` in the detail, `problem` for an
  unreadable file. Keep the list yourself; there is no handle.
- **Convert sheet.** `cf_sheet_new(path)` when the user chooses Convert (refused for an
  unreadable file, as on the desktop). Render `cf_sheet_state`; on every edit send the one
  change and re-render. Enable the Convert button on `can_queue`; show `problems`, `hint`,
  `notes`, `processing_note` and, as a detail, `command_line`. `cf_queue_add(queue, sheet)`
  queues it; destroy the sheet when the screen closes.
- **Queue.** One `cf_queue_new` for the app's life. A background task waits on the revision
  and publishes `cf_queue_state`. Rows offer Cancel (`pending`, `running`), Retry (`failed`,
  `cancelled`; the row keeps its place with a new id), Remove (anything not running), and
  Clear finished. Before the app is terminated, `quit_warning` says what would be lost.
- **Waveform and loudness.** `cf_analysis_new(path)` when a file is shown; wait on the revision,
  read the state, and on each change (and each zoom or scroll) ask `cf_analysis_columns` for the
  visible range at the view's width in pixels. Destroy it (which cancels it) when another file is
  shown, or keep finished ones as a cache.
- **Preview.** One player per output route: `cf_player_new(rate, channels)` with the
  session's output format, `cf_player_take_feed` once, hand the feed to the render callback;
  `cf_player_load` the shown file and drive the transport from the main thread.

### Swift: pulling from `AVAudioSourceNode`

```swift
let session = AVAudioSession.sharedInstance()           // .playback, activated
let rate = UInt32(session.sampleRate), channels = UInt16(2)
var player: OpaquePointer?; cf_player_new(rate, channels, &player)
var feed: OpaquePointer?;   cf_player_take_feed(player, &feed)
let format = AVAudioFormat(standardFormatWithSampleRate: Double(rate),
                           channels: AVAudioChannelCount(channels))!   // deinterleaved
// Allocated once, here: nothing in the render block may allocate.
let planes = UnsafeMutablePointer<UnsafeMutablePointer<Float>?>.allocate(capacity: Int(channels))
let source = AVAudioSourceNode(format: format) { _, _, frames, abl in
    let buffers = UnsafeMutableAudioBufferListPointer(abl)
    for i in 0..<Int(channels) { planes[i] = buffers[i].mData?.assumingMemoryBound(to: Float.self) }
    cf_feed_fill_planar(feed, planes, Int(channels), Int(frames))
    return noErr
}
engine.attach(source); engine.connect(source, to: engine.mainMixerNode, format: format)
```

With an interleaved `AVAudioFormat` call `cf_feed_fill(feed, buffers[0].mData, frames * channels)`
instead. On `AVAudioSession.routeChangeNotification` (or a new output format), stop the
engine, destroy the player and the feed, make new ones at the new format, reload and seek to
the old position. Destroy a feed only after the engine has stopped calling the block.

### Swift: waiting

```swift
Task.detached {                       // never the main actor: the wait blocks
    var seen: UInt64 = 0
    while !Task.isCancelled {
        var now: UInt64 = 0
        cf_queue_wait(queue, seen, 1000, &now)
        if now != seen { seen = now; let json = readState(queue); await MainActor.run { apply(json) } }
    }
}
```

Read and free strings with a helper: `defer { cf_string_free(ptr) }` after
`String(cString: ptr)`; decode with `JSONDecoder` (`.convertFromSnakeCase`).

## Not exposed yet

- Joining files (`concat`) and handing the queue a job file directly: a job comes from a
  sheet, one input to one output.
- The desktop's settings (`appconfig`: default preset, overwrite, themes, keymaps).
- The waveform strip's gesture model (`WaveView` zoom, pan, ruler ticks, position bar),
  `Follow` (the view turning pages with the playhead) and the `Strip` cache: a phone does its
  own gestures and asks for columns. The short-term loudness curve.
- The queue's row wording, the CLI's normalisation note and clip warning sentences: the
  queue state carries the numbers.
- Inspecting with loudness (`probe --loudness`); cancelling a slow `cf_inspect`.
- Dither, constant bitrate and thread-count choices (the desktop sheet does not offer them
  either; jobs run with the default limits, and threads never change the output).
- Localisation: every string is the app layer's English.
- Android packaging: the NDK build of the `.so` and a JNI or JNA layer for Kotlin.
