# Usage: python3 -I scripts/test/release-notes-test.py
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "release-notes.sh"


class ReleaseNotes(unittest.TestCase):
    def test_first_release_uses_default_repository(self):
        with tempfile.TemporaryDirectory() as directory:
            gh = Path(directory) / "gh"
            gh.write_text('#!/bin/sh\n[ "$2" = "repos/dannyota/greennode/releases/generate-notes" ] || exit 1\nprintf "notes\\n"\n')
            gh.chmod(0o755)
            result = subprocess.run(["bash", str(SCRIPT), "v0.1.0"], env={"PATH": directory + os.pathsep + "/usr/bin:/bin"}, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0)
            self.assertEqual(result.stdout, "## What's Changed\n\nFirst release.\n")


if __name__ == "__main__":
    unittest.main()
