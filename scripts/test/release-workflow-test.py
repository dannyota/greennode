import re
import unittest
from pathlib import Path


WORKFLOW = Path(__file__).resolve().parents[2] / ".github/workflows/release.yml"


def validate(text):
    assert re.search(r"(?m)^on:\n  push:\n    tags: \[\"v\*\"\]\n\n", text), "trigger"
    assert re.search(r"(?m)^permissions: \{\}$", text), "default permissions"
    jobs = re.split(r"(?m)^  ([a-z][a-z-]*):\n", text.split("\njobs:\n", 1)[1])
    sections = dict(zip(jobs[1::2], jobs[2::2]))
    assert "release" in sections, "release job"
    for name, section in sections.items():
        permissions = re.findall(r"(?m)^    permissions:\n((?:      [^\n]+\n)+)", section)
        assert len(permissions) == 1, f"{name} permissions"
        writes = re.findall(r"(?m)^      ([\w-]+): write$", permissions[0])
        assert writes == (["contents"] if name == "release" else []), f"{name} write permissions"
        assert not re.search(r"(?m)^    environment:", section), f"{name} environment"


class WorkflowTest(unittest.TestCase):
    def test_release(self):
        validate(WORKFLOW.read_text())

    def test_hostile_mutations(self):
        text = WORKFLOW.read_text()
        cases = (
            ('tags: ["v*"]', 'branches: [master]'),
            ('permissions: {}', 'permissions: read-all'),
            ('      contents: write', '      contents: read'),
            ('      contents: read', '      id-token: write'),
            ('    permissions:\n', '    environment: crates-io\n    permissions:\n'),
        )
        for old, new in cases:
            with self.subTest(old=old):
                self.assertIn(old, text)
                with self.assertRaises(AssertionError):
                    validate(text.replace(old, new, 1))


if __name__ == "__main__":
    unittest.main()
