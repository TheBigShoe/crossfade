#!/bin/bash
# Re-encodes every MP4 fixture with macOS afconvert. Usage:
#   cargo run -p crossfade-format --example mp4_fixtures -- /tmp/mp4src
#   tests/fixtures/mp4/regen.sh /tmp/mp4src
# The README lists what each file exercises.
set -euo pipefail
src=${1:?usage: regen.sh <dir with the .wav files>}
out=$(cd "$(dirname "$0")" && pwd)
for s in mono16 stereo16 stereo24 stereo32 ch3 ch4 ch5 ch6 ch7 ch8 noise16 silence; do
  afconvert --no-filler -f m4af -d alac "$src/$s.wav" "$out/alac-$s.m4a"
done
# -d alac takes 16, 24 or 32 bits from the source; 20 needs the format flag (2 = 20-bit).
afconvert --no-filler -f m4af -d alac/2 "$src/stereo20.wav" "$out/alac-stereo20.m4a"
afconvert --no-filler -f m4af -d aac "$src/stereo16.wav" "$out/aac-stereo16.m4a"
