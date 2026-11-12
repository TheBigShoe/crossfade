#!/bin/sh
# Re-encodes every MP3 fixture from the WAV sources the mp3_fixtures example wrote to $1.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
cargo run -q -p crossfade-format --example mp3_fixtures -- "$1" | while IFS="$(printf '\t')" read -r wav opts; do
    name=$(basename "$wav" .wav)
    # shellcheck disable=SC2086
    lame --quiet $opts "$wav" "$here/$name.mp3"
done
