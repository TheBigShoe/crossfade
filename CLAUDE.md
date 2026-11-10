# Crossfade

An FFmpeg-class audio/video tool in pure Rust: one engine, a CLI (`crossfade`)
and a GPUI desktop app (`crossfade-gui`).

## Rules

1. **TDD, visibly.** No production code without a failing test that demanded
   it. Per behaviour: write the smallest test, run it, see it fail for the
   right reason, commit as `test: <what>` with the trimmed failing output in
   the body; then the least code that passes, committed as `feat:`/`fix:`.
   Refactors are `refactor:` with the suite green. A test commit may leave the
   branch red; the branch tip must be green. Scaffolding with no behaviour
   (manifests, licences, justfile) is `chore:`.
2. **Commits:** `<type>: <what changed>`, types `feat fix refactor docs test
   chore style`. Stage files by name (never `git add .` or `-A`). End every
   message with the `Co-Authored-By:` trailer.
3. **Pure Rust engine.** The engine crates (core, format, codec, dsp, graph)
   may not depend, under any feature, on anything that links a native library
   or compiles C/assembly. Enforced by `engine_is_pure_rust`. Every crate
   carries `#![forbid(unsafe_code)]`.
4. **Dependency direction:** core <- codec, dsp <- format <- graph <- app <-
   cli, ui. A demuxer needs a codec's frame-header parser to split packets (as
   FFmpeg's libavformat uses libavcodec's parsers), so format depends on codec,
   never the reverse. `appconfig` stands alone and only ui uses it.
   `crossfade-testkit` depends only on core and is only a dev-dependency.
   Nothing points the other way. GPUI is imported only through
   `crossfade_ui::prelude`.
5. **Self-contained tests.** `cargo test --workspace` needs no network,
   FFmpeg, audio device or display. No wall-clock timing assertions: use the
   `Clock` trait with a manual clock.
6. Short comments: why, not what. No banners, no restating the code.

## Crates

| Crate | Role |
|---|---|
| `crossfade-core` | Time types, audio description, `Limits`, the engine contract (`Packet`, `AudioFrame`, `Demuxer`, `Muxer`, `Decoder`, `Encoder`) and the I/O seams (`MediaSource`, `MediaSink`, `JobIo`, `Clock`) |
| `crossfade-format` | Containers: WAV reader and writer; `open_demuxer` sniffs, `create_muxer` writes |
| `crossfade-codec` | Codecs: PCM; `decoder_for` / `encoder_for` |
| `crossfade-dsp` | Sample-format conversion (its module doc is the project's rounding rule) |
| `crossfade-graph` | `Job` (plain serialisable data), `Pipeline::step`, `run` with events and cancel, `probe` |
| `crossfade-testkit` | Signal generators, PCM oracle, metrics, faulty I/O, manual clock (dev-only) |
| `crossfade-app` | What every front end shares, GUI-free: `FsIo` (file I/O never lives in the engine), `Request`, command-line rendering, `ThreadRunner`, and the screen models (`Inputs`, `inspector`, `ConvertSheet`, `Queue`) |
| `crossfade-cli` | The `crossfade` binary and a small library (`request_from_args`) the parity tests call: `probe`, `convert` |
| `crossfade-ui` | The desktop app: GPUI views over the `crossfade-app` models, the `crossfade-gui` binary, headless UI tests, `examples/screenshots.rs` |
| `appconfig` | App-agnostic config layer extracted from Serialist (no GPUI, no `crossfade-*` deps): config paths, layered JSONC settings, comment-preserving editor, Zed-format keymaps and themes, watcher |
| `crossfade-audit` | Workspace policy checks, e.g. the purity gate (test-only, unpublished) |

The purity gate lives in its own crate so its `serde_json` dependency and
`cargo metadata` call never touch an engine crate's dependency graph.

## Gate

```sh
just ci        # fmt-check + clippy -D warnings + cargo test --workspace + just conformance
just corpus    # once per clone: fetch the conformance files (needs curl and the network)
just conformance  # tier 3, FLAC corpus vs the frozen results (needs the corpus, ~3 s)
just mobile    # engine crates build for aarch64-apple-ios and aarch64-linux-android
just deny      # licences, advisories, sources (needs the network)
just coverage  # line floors: 87% workspace, 90% crossfade-core
just fuzz-smoke  # 30 s per target; needs nightly and cargo-fuzz
just bench     # criterion; not part of the gate
just run       # the desktop app
just screenshots [names]  # offscreen PNGs in target/screenshots (macOS only)
```

`just ci` must pass before a branch is handed over. Work happens on a task
branch (`m<N>/<topic>`); the lead reviews and merges it into `main` with
`--no-ff`, so `main`'s first-parent history is always green.

`just ci` runs tier 3, so a fresh clone needs `just corpus` once. The tier FAILS,
never skips, when a corpus file is missing (the error says to run `just corpus`);
`CROSSFADE_CORPUS_DIR` points it at a shared cache. Plain `cargo test --workspace`
never needs the corpus: the conformance tests are `#[ignore]`d.

`fuzz/` is its own workspace (libFuzzer is C++) with its own lockfile. Without
nightly, a target still runs unsanitised on the pinned toolchain:
`RUSTC_BOOTSTRAP=1 cargo fuzz run --sanitizer none <target>`.

UI work: views stay thin and render a `crossfade-app` model; test the model
first, then the view with `#[gpui_test]` (headless, no display). A convert
built in the GUI must equal `crossfade_cli::request_from_args` for the same
intent (the parity tests). Review `just screenshots` output before handing over
UI changes. Tests and screenshots never read or write the real config
directory. The first GPUI build takes several minutes; a `target/` cloned from
another checkout (`cp -Rc`) needs `cargo clean -p <each workspace crate>` first
or stale artifacts are reused.

The purity test calls `cargo metadata`, which may fetch crate manifests for
other platforms on its very first run.

## Test tiers

Run on every `cargo test`:

- 0: unit and property tests (`proptest`)
- 1: round trips (encode then decode, mux then demux)
- 2: snapshots (`insta`; review `.snap` files by hand, `cargo-insta` is not
  assumed)

Later milestones:

- 3: conformance against reference vectors (`just conformance`; the frozen results
  in `crates/crossfade-format/tests/conformance/` are rewritten only on purpose,
  with `CROSSFADE_BLESS=1`)
- 4: UI (`#[gpui_test]` runs on every `cargo test`; screenshots are a review
  aid, not a gate)
- 5: fuzzing (`cargo-fuzz`)
- 6: differential against FFmpeg (optional, never a gate for tier 0-2)
- 7: benchmarks
