import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "release-notes.sh"


class ReleaseNotes(unittest.TestCase):
    def run_notes(self, body, comparison):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "body").write_text(body)
            (root / "compare.json").write_text(json.dumps(comparison))
            for name in ("jq", "grep"):
                executable = shutil.which(name)
                self.assertIsNotNone(executable)
                (root / name).symlink_to(executable)
            return subprocess.run(["/bin/bash", str(SCRIPT), str(root / "body"), str(root / "compare.json")], env={"PATH": str(root)}, capture_output=True, text=True)

    def test_first_release(self):
        result = self.run_notes("notes\n", {})
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "## What's Changed\n\nFirst release.\n")

    def test_comparison_uses_file_and_skips_merges(self):
        url = "https://github.com/dannyota/greennode/compare/v0.1.0...v0.2.0"
        data = {"commits": [
            {"sha": "1234567890", "parents": [{}], "commit": {"message": "Fix one\nbody"}},
            {"sha": "abcdef1234", "parents": [{}, {}], "commit": {"message": "Merge"}},
        ]}
        result = self.run_notes(f"**Full Changelog**: {url}\n", data)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, f"## What's Changed\n* Fix one (1234567)\n\n**Full Changelog**: {url}\n")

    def test_invalid_comparison_fails(self):
        result = self.run_notes("https://github.com/dannyota/greennode/compare/a...b\n", {})
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
