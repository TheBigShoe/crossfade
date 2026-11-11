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
3. **Pure Rust engine.** The engine crates (core, flac, loudness, resample, format, codec, dsp, graph)
   may not depend, under any feature, on anything that links a native library
   or compiles C/assembly. Enforced by `engine_is_pure_rust`. Every crate
   carries `#![forbid(unsafe_code)]`.
4. **Dependency direction:** core, flac, loudness, resample <- codec, dsp <- format <- graph <-
   app <- cli, ui. Standalone crates (`crossfade-flac` first, then `crossfade-loudness`) depend on
   nothing in the workspace and not even on core; `codec` wraps them in thin
   adapters. `crossfade-resample` stands alone the same way (only `libm`). A demuxer needs a codec's frame-header parser to split packets (as
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
| `crossfade-core` | Time types, audio description (named layouts: `ChannelLayout::from_name`), `Limits`, the engine contract (`Packet`, `AudioFrame`, `Demuxer`, `Muxer`, `Decoder`, `Encoder`, `Filter`) and the I/O seams (`MediaSource`, `MediaSink`, `JobIo`, `Clock`) |
| `crossfade-format` | Containers: WAV, FLAC and AIFF/AIFF-C readers and writers, an Ogg reader and writer (FLAC inside), tags (`vorbis_comment`, WAV `LIST/INFO`, AIFF text chunks); `open_demuxer` sniffs without seeking, `create_muxer` writes |
| `crossfade-flac` | Standalone FLAC, no dependencies at all: `bits`, `crc`, `md5`, `frame` and `streaminfo` parsing, the verifying `Decoder` (frame in, raw `Block` out; `push_frame` decodes on threads), the `Encoder` (levels 0-8, 4-32 bit, never worse than raw, deterministic at any thread count), the `pool` its threads run on, its own `Error`. Other projects can use it alone |
| `crossfade-loudness` | Standalone loudness and peak meter, only `libm` (so results are the same bits on every platform): BS.1770-4 integrated, momentary and short-term loudness, EBU Tech 3342 loudness range, sample and true peak, any rate 8 kHz-1,048,575 Hz, a normalising-gain helper, its own `Error`. Other projects can use it alone |
| `crossfade-resample` | Standalone sample-rate conversion, only `libm`: windowed-sinc polyphase `Resampler` (any rates 1 to 1,048,575 Hz, `Fast`/`Balanced`/`Best`, streaming, time-aligned, the same bits on every platform and for any chunking), its own `Error`. Other projects can use it alone |
| `crossfade-codec` | The engine's codecs: PCM (little- and big-endian), thin adapters from `crossfade-flac`'s decoder and encoder to the engine's `Decoder` and `Encoder` traits (FLAC takes 8-32 bit integer containers and a true depth of 4 up to the container's, 1-8 channels, up to 1,048,575 Hz); `decoder_for` / `encoder_for` |
| `crossfade-dsp` | Sample-format conversion (its module doc is the project's rounding rule), the filter `Chain`, the filters (one module each, with their `FilterPlan`s; `resample` wraps `crossfade-resample`), `dither` (TPDF, clip count), `level` (dB through `libm`) |
| `crossfade-graph` | `Job` (plain serialisable data), `filter` (the user's filter text and JSON, units, and `plan`, the one mapping to the dsp's plans), `describe`/`validate` and `describe_concat`/`validate_concat` (the I/O-free plan and check `Pipeline::open` shares), `processing`, `Pipeline::step`/`step_with` with transcode (one input or a join) and stream-copy engines, `run` with events and cancel, `probe`, `decode_frames` (a decode-only pass for analysis) |
| `crossfade-testkit` | Signal generators, PCM oracle, metrics, faulty I/O, manual clock (dev-only) |
| `crossfade-app` | What every front end shares, GUI-free: `FsIo` (file I/O never lives in the engine), `Request`, command-line rendering, `ThreadRunner`, and the screen models (`Inputs`, `inspector`, `ConvertSheet`, `Queue`) |
| `crossfade-cli` | The `crossfade` binary and a small library (`request_from_args`) the parity tests call: `probe`, `convert`, `concat` |
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

## Processing (DSP) rules

- **Working format.** Filters process interleaved `f64` frames. The `Chain`
  (in `crossfade-dsp`) converts its input to `f64` with `convert`'s rule, runs
  the filters in order, then quantises to the output format.
- **Bit-exact when nothing is asked.** A job with no filters, no rate or
  channel change and no loss of depth has no chain (`processing` returns
  `None`) and takes the unprocessed path byte for byte; only a chain converts
  to `f64`. Losing depth (`loses_depth`, beside `output_depth`): an integer
  output narrower than the input's true depth, or float to integer; it gets a
  quantise-only chain when the job dithers (`--dither none` keeps the old
  truncation and rounding).
- **Joins** (`crossfade concat`): one input open at a time; each input that
  differs from the joined rate and layout (the first input's, or the output
  settings') gets its own conversion, drained at its end, so seams are
  sample-exact; one main chain runs over the joined stream; inputs alike with
  no processing join bit for bit. `Pipeline::step_with(io)` opens the next
  input.
- **Stopping early.** `Filter::ignores_further_input` (default `false`): once
  any stage of the chain says so, the pipeline stops reading and decoding,
  drains, and reports the whole input as done.
- **Dither and clipping.** After a chain, integer output gets TPDF dither
  (`--dither none` turns it off) from SplitMix64 with a fixed seed, one draw
  per sample in stream order: the same bytes on every platform, at every
  thread count and however frames are split. Integer output saturates; the
  count (signal past full scale, not dither alone) reaches the front end as
  `Event::Clipped`. Float output is never dithered or clipped. Processed
  output's true depth is its format's full width (overriding `output_depth`).
- **Determinism.** Engine DSP uses no `std` transcendental functions (`sin`,
  `cos`, `exp`, `ln`, `log10`, `powf`) and no `mul_add`: `libm` only, so a
  gain is the same bits everywhere. Plain `+ - * /`, `round`, `sqrt` and
  integer powers are exact IEEE operations and fine.
- **Where things live.** The user's words (units, seconds, dB, the text form,
  serde) are `crossfade-graph`'s `filter::FilterSpec`; what runs (frames at
  each filter's own input rate, linear gains) is `crossfade-dsp`'s
  `FilterPlan`. `crossfade_graph::filter::plan` is the one mapping, and it
  appends the output's rate and channel conversions as ordinary filters.
  `crossfade_dsp::filters::build` is the one place a plan becomes a running
  filter.
- **Timing.** A filter's output pts count output frames at its output rate,
  without gaps, starting at its first input pts rescaled (see `Filter`).
- **Adding a filter**: its text, JSON and rules in `crossfade-graph`'s `filter`
  module, its plan and `Filter` in its own module under
  `crates/crossfade-dsp/src/filters/`, one arm in `build_threads`, one arm in
  `filter::plan`'s mapping; test it in `crates/crossfade-dsp/tests/` and
  through the engine, add a row to the plans snapshot and the README's table.
- **Normalising** runs twice (`run`): a measuring pass (the chain up to the
  `normalize` stage, into a meter standing in for the encoder; nothing is
  written) and the conversion, with the stage replaced by the gain the
  measurement gives (`ChainPlan::with_measured_gain`). Events `Measuring`, then
  `Normalized`, then `Started`; each pass is half the progress. The input is
  opened twice through `JobIo`.
- **Notes.** A plan's `notes` say what the job assumes (a downmix of unlabelled
  channels takes FLAC's default layout for their count); the CLI prints them.
- **Threads in DSP.** The resampler converts segments of the stream on up to
  `Limits::max_threads` threads (`std::thread::scope`), bit-identical to one
  thread: segments start on the ratio's input period and each has a pre- and
  post-roll as long as the filter's reach. `1` starts no thread, nor does a
  stream under two segments.
- **Stream copy** refuses filters and any rate or channel change.

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
