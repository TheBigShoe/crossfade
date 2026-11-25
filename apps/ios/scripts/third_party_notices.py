#!/usr/bin/env python3
"""Writes the licence notices of the third-party crates linked into the iOS library
(crossfade-ffi built for aarch64-apple-ios): normal dependencies only, no proc-macros, build
or dev dependencies, and not the workspace's own crates (the app carries the workspace's
licences and crossfade-opus's COPYING itself).

    python3 -I apps/ios/scripts/third_party_notices.py OUTPUT.txt

Needs `cargo` and the crates' sources (a `cargo fetch`, which `cargo build` does anyway).
"""

import glob
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", "..", ".."))
LICENCE_FILES = ("LICENSE*", "LICENCE*", "COPYING*", "NOTICE*", "UNLICENSE*")


def linked(metadata, root_name):
    """The third-party packages a build of `root_name` links, sorted by name."""
    packages = {p["id"]: p for p in metadata["packages"]}
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    members = set(metadata["workspace_members"])
    root = next(p["id"] for p in metadata["packages"] if p["name"] == root_name)

    def is_proc_macro(pid):
        return any("proc-macro" in t["kind"] for t in packages[pid]["targets"])

    seen, stack = set(), [root]
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for d in nodes[pid]["deps"]:
            normal = any(k["kind"] is None for k in d["dep_kinds"])
            if normal and not is_proc_macro(d["pkg"]):
                stack.append(d["pkg"])
    found = [packages[p] for p in seen if p not in members]
    return sorted(found, key=lambda p: (p["name"], p["version"]))


def notice(packages):
    parts = [
        "Third-party software linked into Crossfade's library, with its licence notices.\n"
        "The Rust standard library is also linked: MIT OR Apache-2.0 "
        "(https://github.com/rust-lang/rust).\n"
    ]
    for p in packages:
        parts.append("=" * 72)
        parts.append(f"{p['name']} {p['version']} ({p.get('license') or 'see below'})")
        parts.append("=" * 72)
        folder = os.path.dirname(p["manifest_path"])
        files = sorted({f for pat in LICENCE_FILES for f in glob.glob(os.path.join(folder, pat))})
        if not files:
            parts.append("(no licence file in the package; its licence is named above)\n")
        for path in files:
            with open(path, encoding="utf-8", errors="replace") as f:
                parts.append(f"--- {os.path.basename(path)}\n{f.read().rstrip()}\n")
    return "\n".join(parts) + "\n"


def main(argv):
    if len(argv) != 1:
        print(__doc__, file=sys.stderr)
        return 2
    raw = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--filter-platform", "aarch64-apple-ios"],
        cwd=ROOT, check=True, capture_output=True, text=True,
    ).stdout
    text = notice(linked(json.loads(raw), "crossfade-ffi"))
    os.makedirs(os.path.dirname(os.path.abspath(argv[0])), exist_ok=True)
    with open(argv[0], "w", encoding="utf-8") as f:
        f.write(text)
    print(f"third_party_notices: wrote {argv[0]}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
