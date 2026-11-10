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

# Library-only builds need no linker or NDK. Keep in sync with the CI mobile job.
mobile:
    for t in aarch64-apple-ios aarch64-linux-android; do \
        cargo build --target "$t" -p crossfade-core -p crossfade-format -p crossfade-codec -p crossfade-dsp -p crossfade-graph; \
    done

# Needs the network the first time (advisory database).
deny:
    cargo deny check

# Two runs: the workspace floor, then a stricter floor for core alone.
coverage:
    cargo llvm-cov --workspace --fail-under-lines 87
    cargo llvm-cov -p crossfade-core --fail-under-lines 90

# Dependencies run in order and the first failure stops the recipe.
ci: fmt-check lint test
