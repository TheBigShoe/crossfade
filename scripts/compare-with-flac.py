#!/usr/bin/env python3
"""Times `crossfade convert` against the reference `flac` tool, whole process against whole
process, and prints Markdown tables (see docs/performance.md).

    scripts/compare-with-flac.py [--reps N] [--levels 0,5,8] [--all] [--flac PATH] [WAV ...]

Builds the release `crossfade` binary, makes the synthetic test signals with the
`flac_signals` example, and for each signal and level reports the best wall time of N
interleaved runs. Extra WAV files given on the command line are measured too.

A development aid, not a test: it needs `flac` (1.5 or later for the `-j` column) and the
numbers depend on the machine and on what else it is doing.
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
DEFAULT = ["bench16", "bench24"]
# The other synthetic signals a plain WAV header can describe (16- or 24-bit).
MORE = ["noise16", "noise24", "silence", "tone192", "sweep192", "bl48000_24", "bl96000_24", "bl192000_24"]


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


def audio_bytes(path):
    """Size of a FLAC file without its metadata blocks."""
    with open(path, "rb") as f:
        data = f.read()
    at = 4
    while True:
        last, length = data[at] & 0x80, int.from_bytes(data[at + 1:at + 4], "big")
        at += 4 + length
        if last:
            return len(data) - at


def raw_to_wav(raw, out):
    """`name.RATE.CHANNELS.BITS.raw` (signed little-endian) to a WAV file."""
    _, rate, channels, bits, _ = os.path.basename(raw).split(".")
    with open(raw, "rb") as f, wave.open(out, "wb") as w:
        w.setnchannels(int(channels))
        w.setsampwidth(int(bits) // 8)
        w.setframerate(int(rate))
        w.writeframes(f.read())


def describe(path):
    with wave.open(path, "rb") as w:
        seconds = w.getnframes() / w.getframerate()
        return f"{seconds:.0f} s, {w.getsampwidth() * 8}-bit, {w.getframerate()} Hz, {w.getnchannels()} ch"


def ms(value):
    return "n/a" if value is None else f"{value:.1f}"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--reps", type=int, default=9)
    ap.add_argument("--levels", default="0,5,8")
    ap.add_argument("--all", action="store_true", help="every 16- and 24-bit synthetic signal")
    ap.add_argument("--flac", default=shutil.which("flac"))
    ap.add_argument("wavs", nargs="*")
    args = ap.parse_args()
    if not args.flac:
        sys.exit("flac not found: install it or pass --flac PATH")
    levels = args.levels.split(",")
    flac = args.flac

    subprocess.run(["cargo", "build", "--release", "-p", "crossfade-cli"], cwd=ROOT, check=True)
    with tempfile.TemporaryDirectory() as tmp:
        subprocess.run(
            ["cargo", "run", "--release", "-q", "-p", "crossfade-flac", "--example", "flac_signals", "--", tmp],
            cwd=ROOT, check=True,
        )
        raws = {name.split(".")[0]: os.path.join(tmp, name) for name in os.listdir(tmp) if name.endswith(".raw")}
        cases = []
        for name in DEFAULT + (MORE if args.all else []):
            wav = os.path.join(tmp, name + ".wav")
            raw_to_wav(raws[name], wav)
            cases.append((name, wav))
        cases += [(os.path.splitext(os.path.basename(p))[0], os.path.abspath(p)) for p in args.wavs]

        # `-j` came with flac 1.5; without it that column is left out.
        probe = os.path.join(tmp, "probe.flac")
        threaded = subprocess.run(
            [flac, "-0", "-j2", "-s", "-f", "-o", probe, cases[0][1]],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        ).returncode == 0
        version = subprocess.run([flac, "--version"], capture_output=True, text=True).stdout.strip()
        print(f"Reference: {version}. Best of {args.reps} interleaved runs, wall time in ms.\n")
        for name, wav in cases:
            print(f"- `{name}`: {describe(wav)}")

        ours, ref = os.path.join(tmp, "ours.flac"), os.path.join(tmp, "ref.flac")
        print("\n### Encode (WAV to FLAC)\n")
        print("| Signal | Level | crossfade | crossfade `--threads 1` | flac | flac `-j8` | Size against flac |")
        print("|---|---|---|---|---|---|---|")
        for name, wav in cases:
            for level in levels:
                convert = [CROSSFADE, "convert", wav, "-o", ours, "--compression-level", level, "--force"]
                reference = [flac, f"-{level}", "-s", "-f", "-o", ref, wav]
                t = best([
                    convert,
                    convert + ["--threads", "1"],
                    reference,
                    reference[:2] + ["-j8"] + reference[2:] if threaded else None,
                ], args.reps)
                run(convert)
                run(reference)
                size = (audio_bytes(ours) / audio_bytes(ref) - 1) * 100
                print(f"| {name} | {level} | {ms(t[0])} | {ms(t[1])} | {ms(t[2])} | {ms(t[3])} | {size:+.2f}% |")

        out_ours, out_ref = os.path.join(tmp, "ours.wav"), os.path.join(tmp, "ref.wav")
        print("\n### Decode (FLAC to WAV; the input is the reference encoder's file)\n")
        print("| Signal | Level | crossfade | crossfade `--threads 1` | flac `-d` |")
        print("|---|---|---|---|---|")
        for name, wav in cases:
            for level in levels:
                run([flac, f"-{level}", "-s", "-f", "-o", ref, wav])
                convert = [CROSSFADE, "convert", ref, "-o", out_ours, "--force"]
                t = best([
                    convert,
                    convert + ["--threads", "1"],
                    [flac, "-d", "-s", "-f", "-o", out_ref, ref],
                ], args.reps)
                print(f"| {name} | {level} | {ms(t[0])} | {ms(t[1])} | {ms(t[2])} |")


if __name__ == "__main__":
    main()
