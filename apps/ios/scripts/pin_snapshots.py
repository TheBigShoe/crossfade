#!/usr/bin/env python3
"""Copies the JSON documents pinned in crates/crossfade-ffi/tests/snapshots/shapes__*.snap into
the Swift tests' resources, so the Rust side and the Swift models are held to one truth.

    python3 -I apps/ios/scripts/pin_snapshots.py           # rewrite the copies
    python3 -I apps/ios/scripts/pin_snapshots.py --check   # exit 1 if they drift

The snapshots replace revisions and the meter's figures with strings ("<revision>",
"<number>"); a model decoding numbers needs numbers there, so those become fixed numbers.
"""

import argparse
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", "..", ".."))
SNAPSHOTS = os.path.join(ROOT, "crates", "crossfade-ffi", "tests", "snapshots")
OUT = os.path.join(HERE, "..", "CrossfadeKit", "Tests", "CrossfadeKitTests", "Snapshots")
OUT = os.path.normpath(OUT)

PLACEHOLDERS = {"<revision>": 7, "<number>": -6.5}


def _swap(v):
    if isinstance(v, str) and v in PLACEHOLDERS:
        return PLACEHOLDERS[v]
    if isinstance(v, list):
        return [_swap(x) for x in v]
    if isinstance(v, dict):
        return {k: _swap(x) for k, x in v.items()}
    return v


def payload(snap_text):
    """The document in an insta snapshot: everything after the second `---` line."""
    lines = snap_text.splitlines()
    marks = [i for i, line in enumerate(lines) if line.strip() == "---"]
    if len(marks) < 2:
        raise ValueError("not an insta snapshot: no header")
    return _swap(json.loads("\n".join(lines[marks[1] + 1:])))


def render(doc):
    return json.dumps(doc, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


def expected(snapshots):
    out = {}
    for name in sorted(os.listdir(snapshots)):
        if name.startswith("shapes__") and name.endswith(".snap"):
            with open(os.path.join(snapshots, name), encoding="utf-8") as f:
                doc = payload(f.read())
            out[name[len("shapes__"):-len(".snap")] + ".json"] = render(doc)
    return out


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true", help="fail if the copies drift")
    ap.add_argument("--snapshots", default=SNAPSHOTS)
    ap.add_argument("--out", default=OUT)
    args = ap.parse_args(argv)

    want = expected(args.snapshots)
    if args.check:
        have = set(os.listdir(args.out)) if os.path.isdir(args.out) else set()
        problems = []
        for name, text in want.items():
            path = os.path.join(args.out, name)
            if name not in have:
                problems.append(f"missing {name}")
                continue
            with open(path, encoding="utf-8") as f:
                if f.read() != text:
                    problems.append(f"drifted {name}")
        problems += [f"extra {n}" for n in sorted(have - set(want)) if n.endswith(".json")]
        for p in problems:
            print(f"pin_snapshots: {p} (run apps/ios/scripts/pin_snapshots.py)", file=sys.stderr)
        return 1 if problems else 0

    os.makedirs(args.out, exist_ok=True)
    for name in os.listdir(args.out):
        if name.endswith(".json") and name not in want:
            os.remove(os.path.join(args.out, name))
    for name, text in want.items():
        with open(os.path.join(args.out, name), "w", encoding="utf-8") as f:
            f.write(text)
    print(f"pin_snapshots: wrote {len(want)} files to {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
