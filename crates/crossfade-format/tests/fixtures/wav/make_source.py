#!/usr/bin/env python3
"""Writes the integer source WAV the float fixtures were made from.

usage: make_source.py CHANNELS CHANNEL_MASK OUT.wav
32 frames at 48000 Hz, 16-bit; sample(frame, channel) = frame * 37 + channel * 1000 - 3000.
tests/wav_fixtures.rs regenerates the same values.
"""
import struct
import sys

ch, mask, out = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3]
data = b"".join(
    struct.pack("<h", f * 37 + c * 1000 - 3000) for f in range(32) for c in range(ch)
)
fmt = struct.pack("<HHIIHHHHIH", 0xFFFE, ch, 48000, 48000 * 2 * ch, 2 * ch, 16, 22, 16, mask, 1)
fmt += bytes([0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71])
body = b"WAVE" + b"fmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", len(data)) + data
open(out, "wb").write(b"RIFF" + struct.pack("<I", len(body)) + body)
