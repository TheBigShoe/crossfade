#!/usr/bin/env python3
"""Times `crossfade convert` encoding Apple Lossless and Opus against the reference tools,
whole process against whole process, and prints a Markdown table (see docs/performance.md).

    scripts/compare-encoders.py [--reps N] [--seconds S] [WAV]

Builds the release `crossfade` binary, makes a test signal (the `flac_signals` example's
`bench16`, repeated to S seconds) unless a 16-bit WAV is given, and reports the best wall
time of N interleaved runs of each encoder, with the size of the file each wrote.

A development aid, not a test. References: macOS's `afconvert` and `ffmpeg` for Apple
Lossless, `opusenc` (opus-tools) for Opus. A missing tool leaves its rows out. The numbers
depend on the machine and on what else it is doing.
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
            start = time.perf_counter()
            run(cmd)
            times[i] = min(times[i], time.perf_counter() - start)
    return [t * 1000 for t in times]


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
        return f"{seconds:.0f} s, {w.getsampwidth() * 8}-bit, {w.getframerate()} Hz, {w.getnchannels()} ch", seconds, w.getsampwidth() * 8


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--reps", type=int, default=9)
    ap.add_argument("--seconds", type=int, default=60, help="length of the generated signal")
    ap.add_argument("wav", nargs="?", help="a 16- or 24-bit WAV to use instead of the generated signal")
    args = ap.parse_args()
    afconvert, ffmpeg, opusenc = (shutil.which(t) for t in ("afconvert", "ffmpeg", "opusenc"))
    if not (afconvert or ffmpeg or opusenc):
        sys.exit("none of afconvert, ffmpeg and opusenc found: nothing to compare with")

    subprocess.run(["cargo", "build", "--release", "-p", "crossfade-cli"], cwd=ROOT, check=True)
    with tempfile.TemporaryDirectory() as tmp:
        wav = os.path.abspath(args.wav) if args.wav else bench_signal(tmp, args.seconds)
        path = lambda name: os.path.join(tmp, name)
        # (label, our output, our extra arguments, [(reference name, its command, its output)])
        text, seconds, bits = describe(wav)
        cases = [(f"Apple Lossless, {bits}-bit, M4A", path("ours.m4a"), [], [])]
        if afconvert:
            cases[0][3].append(("afconvert", [afconvert, "-f", "m4af", "-d", "alac", wav, path("af.m4a")], path("af.m4a")))
        if ffmpeg:
            cases[0][3].append(("ffmpeg", [ffmpeg, "-v", "error", "-y", "-i", wav, "-c:a", "alac", path("ff.m4a")], path("ff.m4a")))
        if opusenc:
            for kbps in (64, 128):
                ref = path(f"ref{kbps}.opus")
                cases.append((
                    f"Opus, {kbps} kbit/s", path(f"ours{kbps}.opus"), ["--bitrate", f"{kbps}k"],
                    [("opusenc", [opusenc, "--quiet", "--bitrate", str(kbps), wav, ref], ref)],
                ))

        print(f"Signal: {text}. Best of {args.reps} interleaved runs, wall time in ms,")
        print("sizes in bytes (Opus with its bitrate in kbit/s).\n")
        print("| Output | crossfade | crossfade `--threads 1` | Reference | Reference time | Size, crossfade | Size, reference |")
        print("|---|---|---|---|---|---|---|")
        for label, ours, extra, refs in cases:
            convert = [CROSSFADE, "convert", wav, "-o", ours, "--force"] + extra
            t = best([convert, convert + ["--threads", "1"]] + [cmd for _, cmd, _ in refs], args.reps)
            size = lambda f: f"{os.path.getsize(f)} ({os.path.getsize(f) * 8 / seconds / 1000:.1f})" if f.endswith(".opus") else str(os.path.getsize(f))
            for i, (name, _, out) in enumerate(refs):
                print(f"| {label} | {t[0]:.0f} | {t[1]:.0f} | `{name}` | {t[2 + i]:.0f} | {size(ours)} | {size(out)} |")


if __name__ == "__main__":
    main()
