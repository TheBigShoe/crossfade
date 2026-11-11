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
3. **Pure Rust engine.** The engine crates (core, flac, format, codec, dsp, graph)
   may not depend, under any feature, on anything that links a native library
   or compiles C/assembly. Enforced by `engine_is_pure_rust`. Every crate
   carries `#![forbid(unsafe_code)]`.
4. **Dependency direction:** core, flac <- codec, dsp <- format <- graph <-
   app <- cli, ui. Standalone codec crates (`crossfade-flac` first) depend on
   nothing in the workspace and not even on core; `codec` wraps them in thin
   adapters. A demuxer needs a codec's frame-header parser to split packets (as
   FFmpeg's libavformat uses libavcodec's parsers), so format depends on the
   standalone codec crate directly, never on the engine's `codec`. `appconfig` stands alone and only ui uses it.
   `crossfade-testkit` depends only on core and is only a dev-dependency.
   Nothing points the other way. GPUI is imported only through
   `crossfade_ui::prelude`.
5. **Self-contained tests.** `cargo test --workspace` needs no network,
   FFmpeg, audio device or display. No wall-clock timing assertions: use the
   `Clock` trait with a manual clock.
6. Short comments: why, not what. No banners, no restating the code.
7. **Threads never change output.** A thread count is an execution setting
   (`Limits::max_threads`, `--threads`), never part of a `Job`: the bytes, and
   the errors, are the same at every count, and tests compare 1 thread with
   several (the conformance tier runs both). Threads come from `std` only
   (`crossfade_flac`'s `pool`), and `1` starts none.

## Crates

| Crate | Role |
|---|---|
| `crossfade-core` | Time types, audio description, `Limits`, the engine contract (`Packet`, `AudioFrame`, `Demuxer`, `Muxer`, `Decoder`, `Encoder`) and the I/O seams (`MediaSource`, `MediaSink`, `JobIo`, `Clock`) |
| `crossfade-format` | Containers: WAV, FLAC and AIFF/AIFF-C readers and writers, an Ogg reader (FLAC inside), tags (`vorbis_comment`, WAV `LIST/INFO`, AIFF text chunks); `open_demuxer` sniffs without seeking, `create_muxer` writes |
| `crossfade-flac` | Standalone FLAC, no dependencies at all: `bits`, `crc`, `md5`, `frame` and `streaminfo` parsing, the verifying `Decoder` (frame in, raw `Block` out; `push_frame` decodes on threads), the `Encoder` (levels 0-8, 4-32 bit, never worse than raw, deterministic at any thread count), the `pool` its threads run on, its own `Error`. Other projects can use it alone |
| `crossfade-codec` | The engine's codecs: PCM (little- and big-endian), thin adapters from `crossfade-flac`'s decoder and encoder to the engine's `Decoder` and `Encoder` traits (FLAC takes 8-32 bit integer containers and a true depth of 4 up to the container's, 1-8 channels, up to 1,048,575 Hz); `decoder_for` / `encoder_for` |
| `crossfade-dsp` | Sample-format conversion (its module doc is the project's rounding rule) |
| `crossfade-graph` | `Job` (plain serialisable data), `validate` (the I/O-free check `Pipeline::open` shares), `Pipeline::step` with transcode and stream-copy engines, `run` with events and cancel, `probe` |
| `crossfade-testkit` | Signal generators, PCM oracle, metrics, faulty I/O, manual clock (dev-only) |
| `crossfade-app` | What every front end shares, GUI-free: `FsIo` (file I/O never lives in the engine), `Request`, command-line rendering, `ThreadRunner`, and the screen models (`Inputs`, `inspector`, `ConvertSheet`, `Queue`) |
| `crossfade-cli` | The `crossfade` binary and a small library (`request_from_args`) the parity tests call: `probe`, `convert` |
| `crossfade-ui` | The desktop app: GPUI views over the `crossfade-app` models, the `crossfade-gui` binary, headless UI tests, `examples/screenshots.rs` |
| `appconfig` | App-agnostic config layer extracted from Serialist (no GPUI, no `crossfade-*` deps): config paths, layered JSONC settings, comment-preserving editor, Zed-format keymaps and themes, watcher |
| `crossfade-corpus` | Conformance corpus tool: `corpus.toml` manifest, fetch/seed/verify into `.corpus-cache`, `require(id)` for tier-3 tests (not an engine crate) |
| `crossfade-audit` | Workspace policy checks, e.g. the purity gate (test-only, unpublished) |

Format rules worth knowing before touching a codec or container: decoders
emit samples left-justified in the container sample format and record the
true depth in `StreamInfo::bits_per_raw_sample`, which every writer honours (the one
rule for it is `crossfade-graph`'s `output_depth`); a speaker layout is a
`ChannelLayout` that converts to and from the WAVE channel mask, in storage order; 8-bit signed audio is carried
as `U8` with a +128 offset; a reader never seeks unless the source says it
can, and never allocates a length it read from the file without checking
`Limits`. The reference `flac` tool and macOS `afconvert`/`afinfo` are
development oracles only; committed tests never call an external tool.
Small reference-encoder fixtures live under `crates/*/tests/fixtures/` with a
README giving the command that made each (5 MB budget for the repository).

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
just fuzz-smoke  # 30 s per target; needs cargo-fuzz (nightly optional, see the justfile)
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

Speed claims are made whole process against whole process:
`scripts/compare-with-flac.py` times `crossfade convert` against the reference
`flac` and prints the tables in `docs/performance.md` (update them when the
numbers move). In-process timings alone once hid 20 ms of pipeline overhead.
Check a hot-loop change under `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1` too:
auto-vectorisation must not depend on how the crate is split.

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
