#!/usr/bin/env python3
"""Times `crossfade convert` decoding MP3, AAC and ALAC against the reference tools, whole
process against whole process, and prints a Markdown table (see docs/performance.md).

    scripts/compare-decoders.py [--reps N] [--seconds S] [WAV]

Builds the release `crossfade` binary with the `aac` feature, makes a test signal (the
`flac_signals` example's `bench16`, repeated to S seconds) unless a 16-bit WAV is given,
encodes it with `lame` and `afconvert`, and reports the best wall time of N interleaved
runs of each decoder writing a 16-bit WAV.

A development aid, not a test. References: `mpg123` for MP3, macOS's `afconvert` for MP3,
AAC and ALAC; `lame` only makes the MP3 file. A missing tool leaves its rows or columns
out. The numbers depend on the machine and on what else it is doing.
"""
import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import time
import wave

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CROSSFADE = os.path.join(ROOT, "target", "release", "crossfade")


def run(cmd):
    subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def best(commands, reps):
    """Best wall time in ms of each command, the runs interleaved so load hits all alike."""
    times = [float("inf")] * len(commands)
    for _ in range(reps):
        for i, cmd in enumerate(commands):
            if cmd is None:
                continue
            start = time.perf_counter()
            run(cmd)
            times[i] = min(times[i], time.perf_counter() - start)
    return [None if t == float("inf") else t * 1000 for t in times]


def bench_signal(tmp, seconds):
    """`bench16` from the `flac_signals` example, repeated until it is `seconds` long."""
    subprocess.run(
        ["cargo", "run", "--release", "-q", "-p", "crossfade-flac", "--example", "flac_signals", "--", tmp],
        cwd=ROOT, check=True,
    )
    raw = next(os.path.join(tmp, n) for n in os.listdir(tmp) if n.startswith("bench16.") and n.endswith(".raw"))
    _, rate, channels, bits, _ = os.path.basename(raw).split(".")
    with open(raw, "rb") as f:
        data = f.read()
    frame = int(channels) * int(bits) // 8
    copies = max(1, round(seconds * int(rate) * frame / len(data)))
    out = os.path.join(tmp, "signal.wav")
    with wave.open(out, "wb") as w:
        w.setnchannels(int(channels))
        w.setsampwidth(int(bits) // 8)
        w.setframerate(int(rate))
        for _ in range(copies):
            w.writeframes(data)
    return out


def describe(path):
    with wave.open(path, "rb") as w:
        seconds = w.getnframes() / w.getframerate()
        return f"{seconds:.0f} s, {w.getsampwidth() * 8}-bit, {w.getframerate()} Hz, {w.getnchannels()} ch"


def ms(value):
    return "n/a" if value is None else f"{value:.0f}"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--reps", type=int, default=9)
    ap.add_argument("--seconds", type=int, default=300, help="length of the generated signal")
    ap.add_argument("wav", nargs="?", help="a 16-bit WAV to use instead of the generated signal")
    args = ap.parse_args()
    lame, mpg123, afconvert = (shutil.which(t) for t in ("lame", "mpg123", "afconvert"))
    if not (lame or afconvert):
        sys.exit("neither lame nor afconvert found: nothing can make the test files")

    subprocess.run(["cargo", "build", "--release", "-p", "crossfade-cli", "--features", "aac"], cwd=ROOT, check=True)
    with tempfile.TemporaryDirectory() as tmp:
        wav = os.path.abspath(args.wav) if args.wav else bench_signal(tmp, args.seconds)
        ours, ref = os.path.join(tmp, "ours.wav"), os.path.join(tmp, "ref.wav")
        # (label, the file, how to make it, the reference decoders by name)
        cases = []
        if lame:
            mp3 = os.path.join(tmp, "in.mp3")
            cases.append(("MP3, CBR 192 kbit/s (LAME)", mp3, [lame, "--quiet", "-b", "192", wav, mp3], ["mpg123", "afconvert"]))
        if afconvert:
            aac, m4a, alac = (os.path.join(tmp, n) for n in ("in.aac", "aac.m4a", "alac.m4a"))
            cases += [
                ("AAC-LC, 256 kbit/s, ADTS", aac, [afconvert, "-f", "adts", "-d", "aac", "-b", "256000", wav, aac], ["afconvert"]),
                ("AAC-LC, 192 kbit/s, M4A", m4a, [afconvert, "-f", "m4af", "-d", "aac", "-b", "192000", wav, m4a], ["afconvert"]),
                ("ALAC, M4A", alac, [afconvert, "-f", "m4af", "-d", "alac", wav, alac], ["afconvert"]),
            ]
        decoders = {
            "mpg123": lambda src: [mpg123, "-q", "-w", ref, src] if mpg123 else None,
            "afconvert": lambda src: [afconvert, "-f", "WAVE", "-d", "LEI16", src, ref] if afconvert else None,
        }

        print(f"Signal: {describe(wav)}. Best of {args.reps} interleaved runs, wall time in ms,")
        print("each command decoding to a 16-bit WAV.\n")
        print("| Input | crossfade | crossfade `--threads 1` | mpg123 | afconvert |")
        print("|---|---|---|---|---|")
        for label, src, make, refs in cases:
            run(make)
            convert = [CROSSFADE, "convert", src, "-o", ours, "--force"]
            t = best([
                convert,
                convert + ["--threads", "1"],
                decoders["mpg123"](src) if "mpg123" in refs else None,
                decoders["afconvert"](src) if "afconvert" in refs else None,
            ], args.reps)
            print(f"| {label} | {ms(t[0])} | {ms(t[1])} | {ms(t[2])} | {ms(t[3])} |")


if __name__ == "__main__":
    main()
