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
# lint and test what only that feature compiles (the decoder, the adapter, AAC through the
# readers and the engine, the CLI end to end).
aac_features := "crossfade-aac/decoder,crossfade-codec/aac,crossfade-format/aac,crossfade-graph/aac,crossfade-cli/aac"

lint-aac:
    cargo clippy -p crossfade-aac -p crossfade-codec -p crossfade-format -p crossfade-graph -p crossfade-cli --all-targets --features {{aac_features}} -- -D warnings

test-aac:
    cargo test -p crossfade-aac -p crossfade-codec -p crossfade-format -p crossfade-graph -p crossfade-cli --features {{aac_features}}

# Tier 3: the IETF FLAC corpus, seeking, the MP3 and AAC ISO streams, Xiph's Vorbis files and
# the RFC 8251 Opus vectors, each against frozen results in
# crates/crossfade-format/tests/conformance/. Needs `just corpus` once; fails, never skips,
# without it. Release because the corpus is ~550 MB (about 55 s, Opus's opus_compare most).
# After a deliberate decoder change: CROSSFADE_BLESS=1 just conformance
conformance:
    cargo test --release -p crossfade-format --test conformance_flac -- --ignored
    cargo test --release -p crossfade-format --test conformance_seek -- --ignored
    cargo test --release -p crossfade-format --test conformance_mp3 -- --ignored
    cargo test --release -p crossfade-format --features aac --test conformance_aac -- --ignored
    cargo test --release -p crossfade-format --test conformance_vorbis -- --ignored
    cargo test --release -p crossfade-format --test conformance_opus -- --ignored

# Library-only builds need no linker or NDK. Keep in sync with the CI mobile job. The C binding
# builds as the static library an iOS app links; its cdylib would need the platform's linker.
mobile:
    for t in aarch64-apple-ios aarch64-linux-android; do \
        cargo build --target "$t" -p crossfade-core -p crossfade-format -p crossfade-codec -p crossfade-flac -p crossfade-aac -p crossfade-alac -p crossfade-vorbis -p crossfade-opus -p crossfade-loudness -p crossfade-resample -p crossfade-mp3 -p crossfade-dsp -p crossfade-graph; \
        cargo rustc --target "$t" -p crossfade-ffi --lib --crate-type staticlib; \
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

# The iOS app (apps/ios/README.md). Never built yet: written without a Mac. Everything but
# ios-check needs macOS with Xcode; ios-project also needs XcodeGen (brew install xcodegen).
# IOS_SIMULATOR names the simulator; the default is the first available iPhone.
ios_dir := justfile_directory() / "apps/ios"
ios_notices := ios_dir / "Generated/ThirdPartyNotices.txt"
ios_framework := ios_dir / "CrossfadeKit/Frameworks/CrossfadeFFI.xcframework"
ios_sim_pick := 'sim="${IOS_SIMULATOR:-$(xcrun simctl list devices available | grep -m1 -oE "iPhone[^(]*" | sed "s/ *$//")}"; [[ -n "$sim" ]] || { echo "no iPhone simulator found; install one in Xcode or set IOS_SIMULATOR" >&2; exit 1; }'

_ios-xcode:
    @[[ "$(uname)" == Darwin ]] && command -v xcodebuild >/dev/null 2>&1 || { echo "this recipe needs macOS with Xcode (xcodebuild not found)" >&2; exit 1; }

# The static library for device and simulator as an XCFramework, and the licence notices.
ios-framework: _ios-xcode
    scripts/build-xcframework.sh
    python3 -I apps/ios/scripts/third_party_notices.py "{{ios_notices}}"

# Writes apps/ios/Crossfade.xcodeproj from apps/ios/project.yml.
ios-project: _ios-xcode
    @command -v xcodegen >/dev/null 2>&1 || { echo "ios-project: needs XcodeGen: brew install xcodegen" >&2; exit 1; }
    @[[ -d "{{ios_framework}}" ]] || { echo "ios-project: no XCFramework yet; run: just ios-framework" >&2; exit 1; }
    @[[ -f "{{ios_notices}}" ]] || python3 -I apps/ios/scripts/third_party_notices.py "{{ios_notices}}"
    cd "{{ios_dir}}" && xcodegen generate

# Builds the app for a simulator (unsigned: a simulator needs no team).
ios-build: ios-project
    {{ios_sim_pick}}; \
    xcodebuild -project "{{ios_dir}}/Crossfade.xcodeproj" -scheme Crossfade \
        -destination "platform=iOS Simulator,name=$sim" -derivedDataPath "{{ios_dir}}/build" \
        CODE_SIGNING_ALLOWED=NO build

# Builds, installs and launches the app on a simulator.
ios-run: ios-build
    {{ios_sim_pick}}; \
    app="{{ios_dir}}/build/Build/Products/Debug-iphonesimulator/Crossfade.app"; \
    id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Info.plist")"; \
    xcrun simctl boot "$sim" 2>/dev/null || true; \
    open -a Simulator; \
    xcrun simctl install "$sim" "$app"; \
    xcrun simctl launch "$sim" "$id"

# The wrapper's and the view models' XCTests (the CrossfadeKit package) on a simulator.
ios-test: _ios-xcode
    @[[ -d "{{ios_framework}}" ]] || { echo "ios-test: no XCFramework yet; run: just ios-framework" >&2; exit 1; }
    {{ios_sim_pick}}; \
    cd "{{ios_dir}}/CrossfadeKit" && xcodebuild test -scheme CrossfadeKit-Package \
        -destination "platform=iOS Simulator,name=$sim" -derivedDataPath "{{ios_dir}}/build/package"

# Without a Mac: the Swift tests' JSON against the Rust snapshots, and the scripts' tests.
ios-check:
    python3 -I apps/ios/scripts/pin_snapshots.py --check
    python3 -I -B -m unittest discover -s apps/ios/scripts

# Dependencies run in order and the first failure stops the recipe.
ci: fmt-check lint lint-aac test test-aac conformance

# Needs cargo-fuzz. The default runs unsanitised on the pinned toolchain (RUSTC_BOOTSTRAP lets
# it use libFuzzer's flags); with a nightly toolchain, `FUZZ_RUN="cargo +nightly fuzz run" just
# fuzz <target>` adds AddressSanitizer. `fuzz/corpus` is git-ignored, so each target also reads
# the committed fixtures as an extra corpus directory (the writable corpus is named first, so
# libFuzzer writes new inputs only there): flac_demux and flac_decode -> tests/fixtures/flac,
# aiff_demux -> tests/fixtures/aiff, ogg_demux -> tests/fixtures/ogg, adts_demux ->
# tests/fixtures/adts, mp3_demux and mp3_decode -> tests/fixtures/mp3, mp4_demux ->
# tests/fixtures/mp4; the others start empty.
fuzz_run := env_var_or_default("FUZZ_RUN", "RUSTC_BOOTSTRAP=1 cargo fuzz run --sanitizer none")
fixtures := justfile_directory() / "crates/crossfade-format/tests/fixtures"

fuzz target secs="60":
    seeds=""; \
    case "{{target}}" in \
        flac_demux|flac_decode) seeds="{{fixtures}}/flac" ;; \
        aiff_demux) seeds="{{fixtures}}/aiff" ;; \
        ogg_demux) seeds="{{fixtures}}/ogg" ;; \
        adts_demux) seeds="{{fixtures}}/adts" ;; \
        mp3_demux|mp3_decode) seeds="{{fixtures}}/mp3" ;; \
        mp4_demux) seeds="{{fixtures}}/mp4" ;; \
    esac; \
    mkdir -p fuzz/corpus/{{target}}; \
    cd fuzz && {{fuzz_run}} "{{target}}" corpus/{{target}} $seeds -- -max_total_time={{secs}} -rss_limit_mb=2048 -malloc_limit_mb=256

fuzz-smoke:
    for t in $(cd fuzz && cargo fuzz list); do just fuzz "$t" 30; done
