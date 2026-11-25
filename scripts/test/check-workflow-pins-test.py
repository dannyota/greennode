#!/usr/bin/env -S python3 -I
# Usage: python3 -I scripts/test/check-workflow-pins-test.py
import importlib.util
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "check-workflow-pins.py"
sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location("check_workflow_pins", SCRIPT)
check_workflow_pins = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(check_workflow_pins)

SHA_A = "a" * 40
SHA_B = "b" * 40
PINS = {"rust": "1.100.0", "node": "24.22.3", "cargo-deny": "0.20.2", "cargo-nextest": "0.9.152", "gitleaks": "8.30.1",
        "prettier": "3.10.1", "markdownlint-cli2": "0.24.1"}


class Gate(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="greennode-check-pins."))
        self.addCleanup(shutil.rmtree, self.root)
        (self.root / "scripts").mkdir()
        (self.root / ".github/workflows").mkdir(parents=True)
        shutil.copy(SCRIPT, self.root / "scripts/check-workflow-pins.py")
        self.tools()

    def tools(self, pins=PINS, channel="1.100.0", declared=None, locked=None):
        declared = declared or {"markdownlint-cli2": "0.24.1", "prettier": "3.10.1"}
        locked = locked or declared
        (self.root / ".tool-versions").write_text("".join(f"{k} {v}\n" for k, v in pins.items()))
        (self.root / "rust-toolchain.toml").write_text(f'[toolchain]\nchannel = "{channel}"\n')
        (self.root / "package.json").write_text(json.dumps({"private": True, "devDependencies": declared}))
        packages = {"": {"devDependencies": declared}}
        packages.update({f"node_modules/{k}": {"version": v} for k, v in locked.items()})
        (self.root / "package-lock.json").write_text(json.dumps({"packages": packages}))

    def workflow(self, name, *lines):
        body = "".join(f"      {line}\n" for line in lines)
        (self.root / ".github/workflows" / name).write_text(f"name: x\njobs:\n  a:\n    steps:\n{body}")

    def run_gate(self, *args):
        result = subprocess.run(
            [sys.executable, "-I", str(self.root / "scripts/check-workflow-pins.py"), *args],
            capture_output=True, text=True,
        )
        return result.returncode, (result.stdout + result.stderr).strip()

    def assert_rejected(self, *lines, name="ci.yml", line=5):
        self.workflow(name, *lines)
        code, output = self.run_gate()
        self.assertEqual(code, 1, output)
        self.assertIn(f".github/workflows/{name}:{line}: expected a block 'uses:", output)

    def test_full_pins_pass_and_list(self):
        self.workflow("ci.yml", f"- uses: actions/checkout@{SHA_A} # v7.0.1", "# uses: anything@v1")
        self.workflow(
            "codeql.yaml",
            f"- uses: github/codeql-action/init@{SHA_B} # v4.39.3",
            f"- name: analyze\n        uses: github/codeql-action/analyze@{SHA_B} # v4.39.3",
        )
        self.assertEqual(self.run_gate(), (
            0, "check-workflow-pins: ok (2 actions in 2 files, tool pins agree)",
        ))
        self.assertEqual(self.run_gate("--list"), (0, "\n".join([
            f"actions/checkout 7.0.1 {SHA_A}",
            f"github/codeql-action 4.39.3 {SHA_B}",
        ])))

    def test_bad_block_pins_fail(self):
        for uses in [
            "- uses: actions/checkout@v7",
            "- uses: actions/checkout@v7.0.1 # v7.0.1",
            f"- uses: actions/checkout@{SHA_A[:39]} # v7.0.1",
            f"- uses: actions/checkout@{SHA_A}",
            f"- uses: actions/checkout@{SHA_A} # v7",
            f"- uses: actions/checkout@{SHA_A} # stable",
            f"- uses: actions/checkout@{SHA_A} # v7.0.1 extra",
            f'- uses: "actions/checkout@{SHA_A}" # v7.0.1',
            "- uses: ./local-action",
            "- uses: docker://alpine:3",
            "- uses:",
        ]:
            with self.subTest(uses=uses):
                self.assert_rejected(uses)

    def test_other_key_spellings_fail(self):
        for uses in [
            "- {uses: actions/checkout@v7}",
            f"- {{name: x, uses: actions/checkout@{SHA_A}}}",
            "- [ {uses: actions/checkout@v7} ]",
            '- "uses": actions/checkout@v7',
            "- 'uses': actions/checkout@v7",
            f'- "uses": actions/checkout@{SHA_A} # v7.0.1',
            "- uses : actions/checkout@v7",
            f"- uses : actions/checkout@{SHA_A} # v7.0.1",
            "- uses\t: actions/checkout@v7",
            "- ? uses",
            "  ? 'uses'",
        ]:
            with self.subTest(uses=uses):
                self.assert_rejected(uses)
        self.assert_rejected("- name: x\n        uses: >-\n          actions/checkout@v7", line=6)


    def test_tool_pins_must_agree(self):
        self.workflow("ci.yml", f"- uses: actions/checkout@{SHA_A} # v7.0.1")
        cases = [
            ({"channel": "1.99.0"}, "rust-toolchain.toml says rust 1.99.0, .tool-versions says 1.100.0"),
            ({"declared": {"markdownlint-cli2": "0.24.1", "prettier": "3.10.0"}},
             "package.json says prettier 3.10.0, .tool-versions says 3.10.1"),
            ({"locked": {"markdownlint-cli2": "0.24.0", "prettier": "3.10.1"}},
             "package-lock.json says markdownlint-cli2 0.24.0, .tool-versions says 0.24.1"),
            ({"pins": {k: v for k, v in PINS.items() if k != "node"}}, ".tool-versions: missing node"),
        ]
        for change, message in cases:
            with self.subTest(message=message):
                self.tools(**change)
                code, output = self.run_gate()
                self.assertEqual(code, 1, output)
                self.assertIn(message, output)
        self.tools()
        (self.root / "package-lock.json").write_text(json.dumps({"packages": {
            "": {"devDependencies": {"prettier": "3.10.1"}},
            "node_modules/prettier": {"version": "3.10.1"},
            "node_modules/markdownlint-cli2": {"version": "0.24.1"},
        }}))
        code, output = self.run_gate()
        self.assertEqual(code, 1)
        self.assertIn("package-lock.json devDependencies differ from package.json", output)


if __name__ == "__main__":
    unittest.main()
