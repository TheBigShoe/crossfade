# Crossfade for iPhone and iPad

> **Status: never built, never run.** Everything here was written on a Linux machine with no
> Xcode, no Swift compiler and no simulator. No Swift file in this folder has been compiled,
> no XCTest has run, and the app has never launched. What *was* checked there: the Rust static
> library compiles for `aarch64-apple-ios` (never linked into an app), the JSON the Swift tests
> decode is the Rust side's own (`just ios-check`), every plist, JSON and YAML file parses,
> every shell script passes `bash -n`, and a script balanced the delimiters, imports and type
> names of every Swift file. Expect the first Mac build to need fixes; the list below says
> where to look first.

A native SwiftUI app over Crossfade's shared app layer, through the C interface in
[`crates/crossfade-ffi`](../../crates/crossfade-ffi/README.md). It offers exactly what the
desktop's models offer: the convert form is drawn from the sheet's state and never decides in
Swift what is valid, so it cannot build a conversion the command line would refuse.

## Build and run from a clean Mac

Needs Xcode 15 or later (iOS 17 SDK), Rust (the repository pins 1.98.1), `just` and XcodeGen.

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
brew install just xcodegen
just ios-framework   # the Rust library as CrossfadeFFI.xcframework, and the licence notices
just ios-test        # the Swift package's tests on a simulator (run these first)
just ios-run         # builds the app, boots a simulator, installs and launches it
```

`IOS_SIMULATOR="iPad Pro 13-inch (M4)" just ios-run` picks the simulator; the default is the
first available iPhone. To work in Xcode: `just ios-project && open apps/ios/Crossfade.xcodeproj`
(the project is generated, git-ignored; edit `project.yml`, not the project).

**For a device**, set two things in `Config/Signing.xcconfig`, or better in a new
`Config/Signing.local.xcconfig` beside it (git-ignored, read last):

```
DEVELOPMENT_TEAM = ABCDE12345
PRODUCT_BUNDLE_IDENTIFIER = your.domain.crossfade
```

The simulator needs neither (`ios-build` builds unsigned).

## What is where

| Path | What it is |
|---|---|
| `CrossfadeKit/` | A Swift package, built and tested without the app |
| `CrossfadeKit/Sources/CrossfadeKit` | The only Swift that calls C: Codable models of every JSON document, a final class per handle, `CrossfadeError`, the waits as `AsyncStream`s, `PreviewPlayer` with its `AVAudioEngine` output, and the security-scope helpers (`ScopedAccess`, `ScopeLedger`) |
| `CrossfadeKit/Sources/CrossfadeModels` | The view models (`@Observable`, no SwiftUI): what each screen shows, from CrossfadeKit's protocols |
| `CrossfadeKit/Tests` | XCTests of both; `Snapshots/` are copies of crossfade-ffi's pinned JSON |
| `CrossfadeKit/Frameworks/` | `CrossfadeFFI.xcframework`, written by `just ios-framework` (git-ignored) |
| `Crossfade/` | The app: SwiftUI views, one per file, by screen; `Resources/` holds the Info.plist, the privacy manifest and the asset catalogue |
| `Config/Signing.xcconfig` | Team and bundle identifier (placeholders) |
| `project.yml` | The XcodeGen spec the Xcode project is generated from |
| `scripts/` | `pin_snapshots.py` (copies the Rust snapshots into the tests; `--check` for drift), `third_party_notices.py` (the linked crates' licences), and their tests |
| `Generated/` | `ThirdPartyNotices.txt`, written by `just ios-framework` (git-ignored) |

The dependency chain is thin on purpose: the XCFramework, then `CrossfadeKit`, then
`CrossfadeModels`, then the app. A first build can stop at each step: `just ios-test` builds
the package alone.

## Screens

- **Library** (the sidebar on iPad, the first screen on iPhone): files from the system file
  importer (multiple selection), from Files or another app ("Open in", opened in place when
  the provider allows), or dropped on iPad. Each row shows the inspector's one-line summary, or
  why the file cannot be read. Swipe to remove. Kept between launches as bookmarks in
  Application Support.
- **File**: the waveform (`Canvas` over `cf_analysis_columns`, one lane per channel, one
  combined lane past stereo as the library plans): pinch to zoom, drag to pan once zoomed, tap
  to seek, touch and hold then drag to scrub, the playhead following playback (one position
  read per display frame); the transport (play/pause, ten-second skips, stop, the desktop's
  readout); the loudness line with its warning colour and the analysis progress; the
  inspector's sections as a grouped list.
- **Convert** (a sheet): every row, its options and whether it shows or is enabled come from
  `cf_sheet_state`; each choice sends the documented change; problems, notes and the processing
  note are the library's words; the equivalent command line is shown and copyable; **Add to
  Queue** is enabled only when `can_queue` is true.
- **Queue** (a sheet from the toolbar, with a count of jobs to do): progress, cancel, retry,
  remove, clear finished; a finished file can be shared, shown in Files or added to the
  library; a failure shows the engine's whole message. Haptics confirm a queued, finished or
  failed job.
- **About**: `cf_about`'s lines, and the licences the app carries: `LICENSE-MIT`,
  `LICENSE-APACHE`, `crates/crossfade-opus/COPYING` (copied from the repository at build time)
  and the third-party notices of the crates linked into the library.

## Decisions

- **iOS 17.** The views use `@Observable`, `ContentUnavailableView`, `sensoryFeedback`,
  `MagnifyGesture`, `onChange(of:initial:)` and the `Text` style modifiers of iOS 17;
  `NavigationSplitView` (iOS 16) gives the iPad split and the iPhone stack from one view.
- **Swift 5 language mode** (tools version 5.9, `SWIFT_VERSION = 5.0`): the handles' threading
  is the C library's contract, which Swift 6's strict checking cannot see; the few values that
  cross into detached tasks are marked at that one place.
- **XcodeGen, not a hand-written `.xcodeproj`.** A project file written without Xcode is
  hundreds of lines of cross-referenced identifiers where one mistake stops Xcode opening it;
  the spec is about 60 readable lines, and XcodeGen is a developer tool only (nothing of it ships).
  Its licence is MIT as far as I know; I could not confirm it from the repository's metadata
  (the network blocked GitHub and Homebrew from where this was written): check before relying
  on it.
- **Output goes to the app's Documents folder**, which Files shows as *On My iPhone ›
  Crossfade* (`UIFileSharingEnabled`, `LSSupportsOpeningDocumentsInPlace`). It needs no security
  scope, survives the app, and a finished file can be shared from the queue. The name is the
  library's own default (`song.flac`, `song-s16.wav`, `song-copy.wav`): the library names the
  output beside its input and stops renaming once given a path, so a second, untouched sheet
  supplies the name and the app places it in Documents. Typing a name stops that.
- **Security scopes** are held as crossfade-ffi's README asks: during `cf_inspect` and
  `cf_sheet_new`; for a job until it is no longer pending or running (the queue opens the input
  when the job starts); for an analysis until it stops running; for the player while the file
  is loaded. Files dropped without in-place access are copied to `Documents/Imported`.
- **Background.** While a job is pending or running the screen stays on and the app holds a
  background-task assertion. iOS grants that only briefly (often about 30 seconds) after the app
  leaves the screen. When it runs out the app ends the assertion, as iOS requires, and is
  suspended: the conversion pauses mid-file and carries on when the app is opened again. If iOS
  ends the suspended app (memory pressure, a restart), the job is lost; its partial file, a
  hidden `.name.pid.tmp` in Documents, is deleted at the next launch. There is no background
  audio mode and no background processing task: a long conversion needs the app open.
- **Preview audio**: `.playback` session; an `AVAudioSourceNode` render block that only fills
  a plane table allocated up front and calls `cf_feed_fill_planar` (no allocation, lock or
  Objective-C call); the engine runs only while the player says it is playing. An interruption
  pauses (and resumes if iOS says so); unplugging headphones pauses; an engine configuration
  change rebuilds the player at the new rate, keeping the file, position and play state.
- **Privacy manifest**: the Rust library calls `stat`/`fstat` (file sizes, whether an output
  exists), which Apple lists as file timestamp APIs, so `PrivacyInfo.xcprivacy` declares that
  category with reasons C617.1 (files in the app's container) and 3B52.1 (files the person
  picked). Checked by listing the undefined symbols of the iOS static library: no
  `mach_absolute_time`, no disk-space calls. The app stores nothing in `UserDefaults`.
- **No third-party Swift packages**; the Rust dependencies are permissive (MIT, Apache-2.0,
  BSD-3-Clause for `crossfade-opus`, Zlib and Unlicense among the crates' choices).

## Tests to run first

`just ios-test` runs both test targets of the package on a simulator:

- `CrossfadeKitTests`: `SnapshotDecodingTests` (every model against the Rust snapshots),
  `SheetChangeTests`, `ScopedAccessTests` (starts and stops balance), `LibraryCallTests` (the
  real library: ABI version, errors with messages, an analysis stream ending).
- `CrossfadeModelsTests`: `ConvertFormTests` (sheet state to rows; a problem disables queueing),
  `ConvertModelTests` (output placement, changes, refusals), `QueueModelTests` (queue state to
  row states, scopes held and released, the awake assertion), `LibraryModelTests`,
  `PlayerModelTests`, `AppModelTests`, `WaveViewportTests`, `OutputFolderTests`.

`just ios-check` (no Mac needed) fails when the Swift tests' JSON drifts from
`crates/crossfade-ffi/tests/snapshots`; after a deliberate change there, run
`python3 -I apps/ios/scripts/pin_snapshots.py`.

## Most likely to need fixing on the first build

In rough order of likelihood:

1. **Linking the XCFramework.** `import CrossfadeFFI` comes from the module map the build
   script writes next to the header; a static-library XCFramework as an SwiftPM binary target
   sometimes needs the headers in a subfolder or the module map renamed. If the Rust library
   needs a system library the linker does not add by itself, add it to the `CrossfadeKit`
   target's `linkerSettings`.
2. **Concurrency diagnostics** under a newer Xcode: captures in `Task.detached` and in
   `NotificationCenter` closures (`PreviewPlayer+Session.swift`), `MainActor.assumeIsolated`
   in `KeepAwake.swift`, the player's `onStateChange` closure, method references passed as
   `send:` in the convert views.
3. **SwiftUI signatures**: `Text.foregroundStyle` returning `Text` inside `reduce`
   (`LoudnessReadout.swift`), `onTapGesture(coordinateSpace:)` and `MagnifyGesture`'s
   `startAnchor` (`WaveformView.swift`), `.background(.fill.quaternary, in:)`.
4. **The render block's closure type** in `AudioOutput.swift` (`AVAudioSourceNodeRenderBlock`
   with explicit parameter types), and `cf_feed_fill_planar`'s `float *const *` parameter.
5. **Slow type-checking** of `ConvertForm.init` (many closures in one initialiser): split it
   if Xcode reports the expression is too complex.
6. **XcodeGen spec details**: resources outside the folder (`../../LICENSE-MIT`), the
   `optional` notices file, the privacy manifest added explicitly, `#include?` in the xcconfig.
7. **Test resources**: `Bundle.module` in the test targets with `.copy("Snapshots")`.
8. **Gestures at run time** (not a build error): in a list row, pan, tap, touch-and-hold
   scrub and scrolling may compete; tune the masks in `WaveformView.swift`.
9. **Recipes**: the simulator chosen by name, and the package scheme `CrossfadeKit-Package`.

## Gaps in the C interface

Worked around in Swift, and worth adding to `crossfade-ffi` test-first:

- **Output folder.** The sheet names the output beside the input and has no "output folder"
  setting; the app keeps a second sheet only to read the default name. An `output_dir` change
  (keep the default name, change the folder) would remove it.
- **Labels the state does not carry**: the compression level's options are bare numbers (the
  desktop shows "8 · Smallest"); the preset row has no label when no preset matches (the app
  says "Custom", the desktop's word); the level modes (`off gain normalize`) are listed only in
  the README, and the Adjust fields' units (dB, LUFS, s) not at all.
- **Queue row wording** is not exposed (known): the app composes "Converting · 54%",
  "Done · 4,800 frames · gain −9.1 dB" from the numbers, so it reads differently from the desktop.
- **No pause for a running job**, only cancel, so the app cannot pause cleanly when its
  background time ends; it relies on iOS suspending the process.
- **`cf_inspect` cannot be cancelled**, and takes a path: a slow file provider blocks a
  background task until it answers.
- Also known and listed in crossfade-ffi's README: joining files, settings, the short-term
  loudness curve, `probe --loudness`, localisation.
