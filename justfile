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

# AAC is patent-encumbered, so its decoder is behind the off-by-default feature `aac`: these
# lint and test what only that feature compiles (the decoder, the adapter, the CLI end to end).
aac_features := "crossfade-aac/decoder,crossfade-codec/aac,crossfade-cli/aac"

lint-aac:
    cargo clippy -p crossfade-aac -p crossfade-codec -p crossfade-cli --all-targets --features {{aac_features}} -- -D warnings

test-aac:
    cargo test -p crossfade-aac -p crossfade-codec -p crossfade-cli --features {{aac_features}}

# Tier 3: the IETF FLAC corpus through the reader and decoder, against the frozen results in
# crates/crossfade-format/tests/conformance/flac.txt. Needs `just corpus` once; fails, never
# skips, without it. Release because the corpus is ~300 MB (about 3 s; debug takes ~40 s).
# After a deliberate decoder change: CROSSFADE_BLESS=1 just conformance
conformance:
    cargo test --release -p crossfade-format --test conformance_flac -- --ignored

# Library-only builds need no linker or NDK. Keep in sync with the CI mobile job.
mobile:
    for t in aarch64-apple-ios aarch64-linux-android; do \
        cargo build --target "$t" -p crossfade-core -p crossfade-format -p crossfade-codec -p crossfade-flac -p crossfade-aac -p crossfade-loudness -p crossfade-resample -p crossfade-dsp -p crossfade-graph; \
    done

# Speed baselines: in-memory conversion through the engine, then FLAC decode/encode with
# AIFF and WAV read+decode (crates/crossfade-format/benches).
bench:
    cargo bench -p crossfade-graph
    cargo bench -p crossfade-format

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
# The workspace run builds the AAC decoder (`aac_features`) so that it is counted.
coverage:
    cargo llvm-cov --workspace --features {{aac_features}} --fail-under-lines 87 --ignore-filename-regex '{{coverage_ignore}}'
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
ci: fmt-check lint lint-aac test test-aac conformance

# Needs cargo-fuzz. The default runs unsanitised on the pinned toolchain (RUSTC_BOOTSTRAP lets
# it use libFuzzer's flags); with a nightly toolchain, `FUZZ_RUN="cargo +nightly fuzz run" just
# fuzz <target>` adds AddressSanitizer. `fuzz/corpus` is git-ignored, so each target also reads
# the committed fixtures as an extra corpus directory (the writable corpus is named first, so
# libFuzzer writes new inputs only there): flac_demux and flac_decode -> tests/fixtures/flac,
# aiff_demux -> tests/fixtures/aiff, ogg_demux -> tests/fixtures/ogg; the others start empty.
fuzz_run := env_var_or_default("FUZZ_RUN", "RUSTC_BOOTSTRAP=1 cargo fuzz run --sanitizer none")
fixtures := justfile_directory() / "crates/crossfade-format/tests/fixtures"

fuzz target secs="60":
    seeds=""; \
    case "{{target}}" in \
        flac_demux|flac_decode) seeds="{{fixtures}}/flac" ;; \
        aiff_demux) seeds="{{fixtures}}/aiff" ;; \
        ogg_demux) seeds="{{fixtures}}/ogg" ;; \
    esac; \
    mkdir -p fuzz/corpus/{{target}}; \
    cd fuzz && {{fuzz_run}} "{{target}}" corpus/{{target}} $seeds -- -max_total_time={{secs}} -rss_limit_mb=2048 -malloc_limit_mb=256

fuzz-smoke:
    for t in $(cd fuzz && cargo fuzz list); do just fuzz "$t" 30; done
