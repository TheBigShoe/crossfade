set shell := ["bash", "-euo", "pipefail", "-c"]

default:
    @just --list

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --all-targets -- -D warnings

test:
    cargo test --workspace

# Tier 3: the IETF FLAC corpus through the reader and decoder, against the frozen results in
# crates/crossfade-format/tests/conformance/flac.txt. Needs `just corpus` once; fails, never
# skips, without it. Release because the corpus is ~300 MB (about 3 s; debug takes ~40 s).
# After a deliberate decoder change: CROSSFADE_BLESS=1 just conformance
conformance:
    cargo test --release -p crossfade-format --test conformance_flac -- --ignored

# Library-only builds need no linker or NDK. Keep in sync with the CI mobile job.
mobile:
    for t in aarch64-apple-ios aarch64-linux-android; do \
        cargo build --target "$t" -p crossfade-core -p crossfade-format -p crossfade-codec -p crossfade-dsp -p crossfade-graph; \
    done

# First speed baseline: in-memory conversion through the engine.
bench:
    cargo bench -p crossfade-graph

# Download the conformance corpus into .corpus-cache (once; needs curl and the network).
corpus:
    cargo run --release -p crossfade-corpus -- fetch

corpus-verify:
    cargo run --release -p crossfade-corpus -- verify

# Needs the network the first time (advisory database).
deny:
    cargo deny check

# Binary entry points and examples need a display or a process of their own, so they stay
# out of every line count. Not for code that is merely untested.
coverage_ignore := '(/main\.rs$|/examples/)'

# Two runs: the workspace floor, then a stricter floor for core alone.
coverage:
    cargo llvm-cov --workspace --fail-under-lines 87 --ignore-filename-regex '{{coverage_ignore}}'
    cargo llvm-cov -p crossfade-core --fail-under-lines 90 --ignore-filename-regex '{{coverage_ignore}}'

# Run the desktop app.
run:
    cargo run -p crossfade-ui --bin crossfade-gui

# Render the real workspace offscreen with Metal (macOS only) and write PNGs to
# target/screenshots/. Names pick scenes by file name: `just screenshots 01 light`.
screenshots *names:
    cargo run -p crossfade-ui --example screenshots -- --out target/screenshots {{names}}
    @ls -lh target/screenshots/*.png

# Dependencies run in order and the first failure stops the recipe.
ci: fmt-check lint test conformance

# Needs nightly and cargo-fuzz; not part of `ci`.
fuzz target secs="60":
    cargo +nightly fuzz run {{target}} -- -max_total_time={{secs}} -rss_limit_mb=2048 -malloc_limit_mb=256

fuzz-smoke:
    for t in $(cargo +nightly fuzz list); do \
        cargo +nightly fuzz run "$t" -- -max_total_time=30 -rss_limit_mb=2048 -malloc_limit_mb=256; \
    done
