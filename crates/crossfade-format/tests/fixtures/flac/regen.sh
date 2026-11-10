#!/bin/bash
# Re-encodes every fixture with the reference encoder (flac 1.5.0). Usage:
#   cargo run -p crossfade-format --example flac_fixtures -- /tmp/raw
#   tests/fixtures/flac/regen.sh /tmp/raw
# The README lists what each file exercises.
set -euo pipefail
raw=${1:?usage: regen.sh <dir with the .raw files>}
out=$(cd "$(dirname "$0")" && pwd)
enc() { # <signal> <rate> <channels> <bps> <file> [flags...]
  local sig=$1 rate=$2 ch=$3 bps=$4 file=$5
  shift 5
  flac -s -f --no-padding --force-raw-format --endian=little --sign=signed \
    --sample-rate="$rate" --channels="$ch" --bps="$bps" "$@" -o "$out/$file" "$raw/$sig.raw"
}

enc mono8 8000 1 8 mono-8bit.flac
flac -s -f --no-padding "$raw/stereo12.wav" -o "$out/stereo-12bit.flac" # 12-bit input goes in as WAV
enc stereo16 44100 2 16 stereo-16bit.flac
flac -s -f --no-padding "$raw/stereo20.wav" -o "$out/stereo-20bit.flac"
enc stereo24 96000 2 24 stereo-24bit.flac
enc stereo32 48000 2 32 stereo-32bit.flac
enc surround6 48000 6 16 surround-5.1.flac
enc fixed-orders 48000 1 24 fixed-only.flac -l 0 -b 192 -r 4
enc stereo16 44100 2 16 lpc-order-32.flac --lax -l 32 -b 4096
enc noise16 44100 2 16 verbatim-noise.flac
enc silence 44100 1 16 constant-silence.flac -b 4608
enc wasted 44100 2 16 wasted-bits.flac
enc stereo-modes 44100 2 16 stereo-modes-m.flac -m -b 1024 -l 0
enc stereo-modes 44100 2 16 stereo-modes-adaptive.flac -M -b 1024 -l 0
enc stereo-modes 44100 2 16 stereo-modes-none.flac --no-mid-side -b 1024 -l 0
enc stereo16 44100 2 16 block-16.flac -b 16 --lax
enc stereo16 44100 2 16 block-4608.flac -b 4608
enc stereo16 44100 2 16 block-65535.flac --lax -b 65535
enc stereo16 44100 2 16 block-1000.flac -b 1000
enc stereo16 44100 2 16 tagged.flac --padding=300 -S 4x -T TITLE=Fixture -T ARTIST=Crossfade \
  --picture="3|image/gif|front|1x1x8|$out/pixel.gif"
# A 10-byte ID3v2.4 header (syncsafe size 11) and an 11-byte frame body, then the stream.
{ printf 'ID3\x04\x00\x00\x00\x00\x00\x0b'; printf 'TIT2\x00\x00\x00\x01\x00\x00\x00'; cat "$out/stereo-16bit.flac"; } \
  >"$out/id3v2-prefixed.flac"
# Piped input: the encoder cannot rewrite STREAMINFO, so total samples and MD5 stay zero.
flac -s -c --no-padding --force-raw-format --endian=little --sign=signed --sample-rate=44100 \
  --channels=2 --bps=16 - <"$raw/stereo16.raw" >"$out/unknown-total.flac"
