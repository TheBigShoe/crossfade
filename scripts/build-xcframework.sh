#!/usr/bin/env bash
# Builds CrossfadeFFI.xcframework from crossfade-ffi for the iOS app: the device (arm64) and the
# simulator (arm64 and x86_64, joined with lipo), each with the hand-written header and a
# module map, so Swift can `import CrossfadeFFI`. apps/ios/CrossfadeKit/Package.swift names it
# as a binary target at the default output path below, which .gitignore keeps out of git.
#
# NOT YET RUN: written on a machine without Xcode. The first person to run it on a Mac
# should check the result in the app and remove this line.
#
# Needs macOS with Xcode (xcodebuild, lipo) and the Rust targets:
#   rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
#
# Usage: scripts/build-xcframework.sh [output-dir]   (default: apps/ios/CrossfadeKit/Frameworks)
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/apps/ios/CrossfadeKit/Frameworks}"
header="$root/crates/crossfade-ffi/include/crossfade.h"
lib=libcrossfade_ffi.a
targets=(aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios)
# The app's deployment target (apps/ios/project.yml); rustc reads it for Apple targets, so the
# objects are not marked for a newer iOS than the app.
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"

if [[ "$(uname)" != Darwin ]] || ! command -v xcodebuild >/dev/null 2>&1; then
    echo "build-xcframework.sh: needs macOS with Xcode (xcodebuild not found)" >&2
    exit 1
fi
if ! command -v lipo >/dev/null 2>&1; then
    echo "build-xcframework.sh: lipo not found; install Xcode's command line tools" >&2
    exit 1
fi
if command -v rustup >/dev/null 2>&1; then
    installed="$(rustup target list --installed)"
    for target in "${targets[@]}"; do
        if ! grep -qx "$target" <<<"$installed"; then
            echo "build-xcframework.sh: missing Rust target $target; run:" >&2
            echo "  rustup target add ${targets[*]}" >&2
            exit 1
        fi
    done
fi

cd "$root"
# Static library only: an XCFramework of static archives needs no signing or install name.
for target in "${targets[@]}"; do
    cargo rustc --release --target "$target" -p crossfade-ffi --lib --crate-type staticlib
done

staging="$out/staging"
rm -rf "$out/CrossfadeFFI.xcframework" "$staging"
mkdir -p "$staging/headers" "$staging/simulator"

cp "$header" "$staging/headers/"
cat > "$staging/headers/module.modulemap" <<'EOF'
module CrossfadeFFI {
    header "crossfade.h"
    export *
}
EOF

lipo -create \
    "target/aarch64-apple-ios-sim/release/$lib" \
    "target/x86_64-apple-ios/release/$lib" \
    -output "$staging/simulator/$lib"

xcodebuild -create-xcframework \
    -library "target/aarch64-apple-ios/release/$lib" -headers "$staging/headers" \
    -library "$staging/simulator/$lib" -headers "$staging/headers" \
    -output "$out/CrossfadeFFI.xcframework"

rm -rf "$staging"
echo "wrote $out/CrossfadeFFI.xcframework"
