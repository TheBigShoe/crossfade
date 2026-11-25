#!/usr/bin/env python3
"""Development aid, not a test: drives crossfade-ffi's dynamic library from outside Rust with
ctypes, one whole journey (inspect, sheet, queue, wait, analysis, player pull), to show the
hand-written header matches the library. The signatures below are copied from
crates/crossfade-ffi/include/crossfade.h.

    cargo build -p crossfade-ffi
    python3 -I scripts/ffi-journey.py [path/to/libcrossfade_ffi.so]
"""

import ctypes as C
import json
import math
import os
import struct
import sys
import tempfile
import wave

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT = os.path.join(HERE, "..", "target", "debug", "libcrossfade_ffi.so")

u8p = C.POINTER(C.c_char_p)
handle = C.c_void_p


def load(path):
    lib = C.CDLL(path)
    sig = {
        "cf_abi_version": (C.c_uint32, []),
        "cf_version": (C.c_char_p, []),
        "cf_about": (C.c_int32, [C.POINTER(C.c_void_p)]),
        "cf_string_free": (None, [C.c_void_p]),
        "cf_last_error_message": (C.c_void_p, []),
        "cf_inspect": (C.c_int32, [C.c_char_p, C.POINTER(C.c_void_p)]),
        "cf_sheet_new": (C.c_int32, [C.c_char_p, C.POINTER(handle)]),
        "cf_sheet_apply": (C.c_int32, [handle, C.c_char_p]),
        "cf_sheet_state": (C.c_int32, [handle, C.POINTER(C.c_void_p)]),
        "cf_sheet_destroy": (None, [handle]),
        "cf_queue_new": (C.c_int32, [C.POINTER(handle)]),
        "cf_queue_add": (C.c_int32, [handle, handle, C.POINTER(C.c_uint64)]),
        "cf_queue_state": (C.c_int32, [handle, C.POINTER(C.c_void_p)]),
        "cf_queue_wait": (C.c_int32, [handle, C.c_uint64, C.c_uint32, C.POINTER(C.c_uint64)]),
        "cf_queue_destroy": (None, [handle]),
        "cf_analysis_new": (C.c_int32, [C.c_char_p, C.POINTER(handle)]),
        "cf_analysis_state": (C.c_int32, [handle, C.POINTER(C.c_void_p)]),
        "cf_analysis_wait": (C.c_int32, [handle, C.c_uint64, C.c_uint32, C.POINTER(C.c_uint64)]),
        "cf_analysis_columns": (
            C.c_int32,
            [handle, C.c_uint32, C.c_uint64, C.c_uint64, C.POINTER(C.c_float),
             C.POINTER(C.c_float), C.c_size_t, C.POINTER(C.c_size_t)],
        ),
        "cf_analysis_destroy": (None, [handle]),
        "cf_player_new": (C.c_int32, [C.c_uint32, C.c_uint16, C.POINTER(handle)]),
        "cf_player_take_feed": (C.c_int32, [handle, C.POINTER(handle)]),
        "cf_player_load": (C.c_int32, [handle, C.c_char_p]),
        "cf_player_play": (C.c_int32, [handle]),
        "cf_player_settle": (C.c_int32, [handle]),
        "cf_player_state": (C.c_int32, [handle, C.POINTER(C.c_void_p)]),
        "cf_player_destroy": (C.c_int32, [handle]),
        "cf_feed_fill": (C.c_int32, [handle, C.POINTER(C.c_float), C.c_size_t]),
        "cf_feed_fill_planar": (
            C.c_int32, [handle, C.POINTER(C.POINTER(C.c_float)), C.c_size_t, C.c_size_t]),
        "cf_feed_channels": (C.c_int32, [handle, C.POINTER(C.c_size_t)]),
        "cf_feed_destroy": (None, [handle]),
    }
    for name, (res, args) in sig.items():
        f = getattr(lib, name)
        f.restype = res
        f.argtypes = args
    return lib


def take(lib, ptr):
    text = C.string_at(ptr).decode("utf-8")
    lib.cf_string_free(ptr)
    return text


def check(lib, status, what):
    if status != 0:
        msg = lib.cf_last_error_message()
        raise SystemExit(f"{what}: status {status}: {take(lib, msg) if msg else ''}")


def json_of(lib, fn, *args):
    out = C.c_void_p()
    check(lib, fn(*args, C.byref(out)), fn.__name__)
    return json.loads(take(lib, out.value))


def write_wav(path, rate=48000, seconds=2):
    with wave.open(path, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(rate)
        frames = bytearray()
        for i in range(rate * seconds):
            v = int(16000 * math.sin(2 * math.pi * 440 * i / rate))
            frames += struct.pack("<hh", v, v)
        w.writeframes(bytes(frames))


def main():
    lib = load(sys.argv[1] if len(sys.argv) > 1 else DEFAULT)
    assert lib.cf_abi_version() == 1
    print("version", lib.cf_version().decode(), "abi", lib.cf_abi_version())
    print("about", json_of(lib, lib.cf_about)["lines"][0])

    work = tempfile.mkdtemp(prefix="crossfade-ffi-journey-")
    src = os.path.join(work, "tone.wav")
    write_wav(src)
    facts = json_of(lib, lib.cf_inspect, src.encode())
    print("inspect", facts["summary"])

    sheet = handle()
    check(lib, lib.cf_sheet_new(src.encode(), C.byref(sheet)), "cf_sheet_new")
    status = lib.cf_sheet_apply(sheet, b'{"colour": "red"}')
    msg = lib.cf_last_error_message()
    print("bad change", status, take(lib, msg))
    assert status == 3
    check(lib, lib.cf_sheet_apply(sheet, b'{"container": "flac", "compression_level": 5}'),
          "cf_sheet_apply")
    state = json_of(lib, lib.cf_sheet_state, sheet)
    print("sheet", state["can_queue"], state["command_line"])

    queue = handle()
    check(lib, lib.cf_queue_new(C.byref(queue)), "cf_queue_new")
    job = C.c_uint64()
    check(lib, lib.cf_queue_add(queue, sheet, C.byref(job)), "cf_queue_add")
    lib.cf_sheet_destroy(sheet)
    while True:
        qs = json_of(lib, lib.cf_queue_state, queue)
        row = qs["jobs"][0]
        if row["state"] not in ("pending", "running"):
            break
        rev = C.c_uint64()
        check(lib, lib.cf_queue_wait(queue, qs["revision"], 10000, C.byref(rev)), "wait")
    print("job", row["state"], row["frames"], "frames ->", os.path.basename(row["output"]))
    with open(row["output"], "rb") as f:
        assert f.read(4) == b"fLaC"
    lib.cf_queue_destroy(queue)

    an = handle()
    check(lib, lib.cf_analysis_new(row["output"].encode(), C.byref(an)), "cf_analysis_new")
    while True:
        st = json_of(lib, lib.cf_analysis_state, an)
        if st["state"] != "running":
            break
        rev = C.c_uint64()
        check(lib, lib.cf_analysis_wait(an, st["revision"], 10000, C.byref(rev)), "wait")
    n = 8
    mins, maxs, written = (C.c_float * n)(), (C.c_float * n)(), C.c_size_t()
    fpc = -(-st["frames"] // n)
    check(lib, lib.cf_analysis_columns(an, 0, 0, fpc, mins, maxs, n, C.byref(written)), "cols")
    print("analysis", st["readout"]["text"], "columns", written.value,
          [round(x, 3) for x in maxs])
    lib.cf_analysis_destroy(an)

    player = handle()
    check(lib, lib.cf_player_new(44100, 2, C.byref(player)), "cf_player_new")
    feed = handle()
    check(lib, lib.cf_player_take_feed(player, C.byref(feed)), "take_feed")
    check(lib, lib.cf_player_load(player, row["output"].encode()), "load")
    check(lib, lib.cf_player_settle(player), "settle")
    check(lib, lib.cf_player_play(player), "play")
    check(lib, lib.cf_player_settle(player), "settle")
    buf = (C.c_float * 2048)()
    check(lib, lib.cf_feed_fill(feed, buf, 2048), "fill")
    left, right = (C.c_float * 512)(), (C.c_float * 512)()
    planes = (C.POINTER(C.c_float) * 2)(left, right)
    check(lib, lib.cf_feed_fill_planar(feed, planes, 2, 512), "fill_planar")
    peak = max(abs(x) for x in buf)
    print("player", json_of(lib, lib.cf_player_state, player)["transport"],
          "peak", round(peak, 3), "planar peak", round(max(abs(x) for x in left), 3))
    assert peak > 0.1
    check(lib, lib.cf_player_destroy(player), "destroy")
    lib.cf_feed_destroy(feed)
    for name in os.listdir(work):
        os.remove(os.path.join(work, name))
    os.rmdir(work)
    print("journey complete")


if __name__ == "__main__":
    main()
