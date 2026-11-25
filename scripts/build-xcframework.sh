#!/usr/bin/env bash
# Builds Crossfade.xcframework from crossfade-ffi for an iOS app: the device (arm64) and the
# simulator (arm64 and x86_64, joined with lipo), each with the hand-written header and a
# module map, so Swift can `import CrossfadeFFI`.
#
# NOT YET RUN: written on a machine without Xcode. The first person to run it on a Mac
# should check the result in an app and remove this line.
#
# Needs macOS with Xcode (xcodebuild, lipo) and the Rust targets:
#   rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
#
# Usage: scripts/build-xcframework.sh [output-dir]   (default: target/xcframework)
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/target/xcframework}"
header="$root/crates/crossfade-ffi/include/crossfade.h"
lib=libcrossfade_ffi.a

if [[ "$(uname)" != Darwin ]]; then
    echo "build-xcframework.sh: needs macOS with Xcode" >&2
    exit 1
fi

cd "$root"
# Static library only: an XCFramework of static archives needs no signing or install name.
for target in aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios; do
    cargo rustc --release --target "$target" -p crossfade-ffi --lib --crate-type staticlib
done

staging="$out/staging"
rm -rf "$out/Crossfade.xcframework" "$staging"
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
    -output "$out/Crossfade.xcframework"

rm -rf "$staging"
echo "wrote $out/Crossfade.xcframework"
