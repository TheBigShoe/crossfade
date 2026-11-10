# Crossfade

An FFmpeg-class audio/video tool in pure Rust: one engine, a CLI now, a GPUI
desktop app later.

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
4. **Dependency direction:** core <- format, codec, dsp <- graph <- cli (later
   ui). `crossfade-testkit` depends only on core and is only a dev-dependency.
   Nothing points the other way.
5. **Self-contained tests.** `cargo test --workspace` needs no network,
   FFmpeg, audio device or display. No wall-clock timing assertions: use the
   `Clock` trait with a manual clock.
6. Short comments: why, not what. No banners, no restating the code.

## Crates

| Crate | Role |
|---|---|
| `crossfade-core` | Time (`Rational`, `TimeBase`, `Timestamp`), audio description, `Limits`, I/O and clock seams |
| `crossfade-format` | Container demuxers and muxers (stub) |
| `crossfade-codec` | Decoders and encoders (stub) |
| `crossfade-dsp` | Resampling, format/channel conversion, gain (stub) |
| `crossfade-graph` | Wires the above into pipelines; the engine entry point (stub) |
| `crossfade-testkit` | Signal generators, PCM oracle, metrics, faulty I/O, manual clock (dev-only) |
| `crossfade-cli` | The `crossfade` binary |
| `crossfade-audit` | Workspace policy checks, e.g. the purity gate (test-only, unpublished) |

The purity gate lives in its own crate so its `serde_json` dependency and
`cargo metadata` call never touch an engine crate's dependency graph.

## Gate

```sh
just ci        # fmt-check + clippy -D warnings + cargo test --workspace
just mobile    # engine crates build for aarch64-apple-ios and aarch64-linux-android
just deny      # licences, advisories, sources (needs the network)
just coverage  # line floors: 87% workspace, 90% crossfade-core
just fuzz-smoke  # 30 s per target; needs nightly and cargo-fuzz
```

`just ci` must pass before a branch is handed over. Work happens on a task
branch (`m<N>/<topic>`); the lead reviews and merges it into `main` with
`--no-ff`, so `main`'s first-parent history is always green.

`fuzz/` is its own workspace (libFuzzer is C++) with its own lockfile. Without
nightly, a target still runs unsanitised on the pinned toolchain:
`RUSTC_BOOTSTRAP=1 cargo fuzz run --sanitizer none <target>`.

The purity test calls `cargo metadata`, which may fetch crate manifests for
other platforms on its very first run.

## Test tiers

Run on every `cargo test`:

- 0: unit and property tests (`proptest`)
- 1: round trips (encode then decode, mux then demux)
- 2: snapshots (`insta`; review `.snap` files by hand, `cargo-insta` is not
  assumed)

Later milestones:

- 3: conformance against reference vectors
- 4: UI
- 5: fuzzing (`cargo-fuzz`)
- 6: differential against FFmpeg (optional, never a gate for tier 0-2)
- 7: benchmarks
