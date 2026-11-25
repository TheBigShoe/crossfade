"""Tests for the iOS app's helper scripts. Run from the repository root:

    python3 -I -B -m unittest discover -s apps/ios/scripts
"""

import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import pin_snapshots  # noqa: E402
import third_party_notices  # noqa: E402

SNAP = """---
source: crates/crossfade-ffi/tests/shapes.rs
expression: "shown(state, &root)"
---
{
  "revision": "<revision>",
  "loudness": {"integrated": "<number>", "sample_peak": "<number>"},
  "path": "<dir>/in.wav"
}
"""


class PinSnapshots(unittest.TestCase):
    def test_payload_drops_the_header_and_makes_placeholders_numbers(self):
        v = pin_snapshots.payload(SNAP)
        self.assertIsInstance(v["revision"], int)
        self.assertIsInstance(v["loudness"]["integrated"], float)
        self.assertIsInstance(v["loudness"]["sample_peak"], float)
        # Path placeholders are strings already and stay as the snapshot has them.
        self.assertEqual(v["path"], "<dir>/in.wav")

    def _dirs(self):
        root = tempfile.mkdtemp()
        snaps = os.path.join(root, "snaps")
        out = os.path.join(root, "out")
        os.makedirs(snaps)
        with open(os.path.join(snaps, "shapes__analysis_shape.snap"), "w") as f:
            f.write(SNAP)
        return snaps, out

    def test_write_then_check_passes(self):
        snaps, out = self._dirs()
        self.assertEqual(pin_snapshots.main(["--snapshots", snaps, "--out", out]), 0)
        self.assertTrue(os.path.exists(os.path.join(out, "analysis_shape.json")))
        self.assertEqual(pin_snapshots.main(["--check", "--snapshots", snaps, "--out", out]), 0)

    def test_check_fails_when_a_copy_drifts(self):
        snaps, out = self._dirs()
        pin_snapshots.main(["--snapshots", snaps, "--out", out])
        with open(os.path.join(out, "analysis_shape.json"), "a") as f:
            f.write(" ")
        self.assertEqual(pin_snapshots.main(["--check", "--snapshots", snaps, "--out", out]), 1)

    def test_check_fails_on_a_missing_or_extra_copy(self):
        snaps, out = self._dirs()
        pin_snapshots.main(["--snapshots", snaps, "--out", out])
        with open(os.path.join(out, "stale.json"), "w") as f:
            f.write("{}\n")
        self.assertEqual(pin_snapshots.main(["--check", "--snapshots", snaps, "--out", out]), 1)
        os.remove(os.path.join(out, "stale.json"))
        os.remove(os.path.join(out, "analysis_shape.json"))
        self.assertEqual(pin_snapshots.main(["--check", "--snapshots", snaps, "--out", out]), 1)

    def test_every_output_folder_gets_the_copies_and_is_checked(self):
        snaps, out = self._dirs()
        second = out + "-models"
        args = ["--snapshots", snaps, "--out", out, "--out", second]
        self.assertEqual(pin_snapshots.main(args), 0)
        self.assertTrue(os.path.exists(os.path.join(out, "analysis_shape.json")))
        self.assertTrue(os.path.exists(os.path.join(second, "analysis_shape.json")))
        self.assertEqual(pin_snapshots.main(["--check"] + args), 0)
        os.remove(os.path.join(out, "analysis_shape.json"))
        self.assertEqual(pin_snapshots.main(["--check"] + args), 1)

    def test_the_committed_copies_match_the_rust_snapshots(self):
        self.assertEqual(pin_snapshots.main(["--check"]), 0)


def package(name, manifest_dir, proc_macro=False, license="MIT"):
    kind = ["proc-macro"] if proc_macro else ["lib"]
    return {
        "id": name + "-id",
        "name": name,
        "version": "1.0.0",
        "license": license,
        "manifest_path": os.path.join(manifest_dir, "Cargo.toml"),
        "targets": [{"kind": kind}],
    }


def dep(name, kind=None):
    return {"pkg": name + "-id", "dep_kinds": [{"kind": kind, "target": None}]}


class ThirdPartyNotices(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        os.makedirs(os.path.join(self.dir, "lib"))
        with open(os.path.join(self.dir, "lib", "LICENSE-MIT"), "w") as f:
            f.write("MIT text of lib\n")
        d = self.dir
        self.metadata = {
            "workspace_members": ["ffi-id", "app-id"],
            "packages": [
                package("ffi", d), package("app", d), package("lib", os.path.join(d, "lib")),
                package("deep", d), package("macro", d, proc_macro=True),
                package("macro-dep", d), package("devonly", d), package("buildonly", d),
            ],
            "resolve": {"nodes": [
                {"id": "ffi-id", "deps": [dep("app"), dep("devonly", "dev")]},
                {"id": "app-id", "deps": [dep("lib"), dep("macro"), dep("buildonly", "build")]},
                {"id": "lib-id", "deps": [dep("deep")]},
                {"id": "deep-id", "deps": []},
                {"id": "macro-id", "deps": [dep("macro-dep")]},
                {"id": "macro-dep-id", "deps": []},
                {"id": "devonly-id", "deps": []},
                {"id": "buildonly-id", "deps": []},
            ]},
        }

    def test_only_linked_third_party_crates(self):
        names = [p["name"] for p in third_party_notices.linked(self.metadata, "ffi")]
        self.assertEqual(names, ["deep", "lib"])

    def test_notice_carries_each_licence_file(self):
        text = third_party_notices.notice(third_party_notices.linked(self.metadata, "ffi"))
        self.assertIn("lib 1.0.0 (MIT)", text)
        self.assertIn("MIT text of lib", text)
        self.assertIn("deep 1.0.0 (MIT)", text)


if __name__ == "__main__":
    unittest.main()
