import contextlib
import hashlib
import importlib.util
import io
import json
import tarfile
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "check-crate-release.py"
sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location("check_crate_release", SCRIPT)
check = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(check)


class Manifest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        for name in ("greennode-sdk", "greennode"):
            directory = self.root / "crates" / name
            directory.mkdir(parents=True)
            (directory / "Cargo.toml").write_text(
                f'[package]\nname = "{name}"\nversion.workspace = true\n', encoding="utf-8"
            )
        (self.root / "Cargo.toml").write_text(
            '[workspace.package]\nversion = "0.1.0"\n', encoding="utf-8"
        )

    def test_unimplemented_commands_fail_closed(self):
        self.assertNotEqual(check.main(["receipt"]), 0)

    def test_manifest_command(self):
        self.assertEqual(check.main(["manifest", "--tag", "v0.1.0"], root=self.root), 0)
        self.assertNotEqual(check.main(["manifest", "--tag", "v0.2.0"], root=self.root), 0)

    def test_shipped_command(self):
        candidate = self.root / "candidate.json"
        committed = self.root / "committed.json"
        candidate.write_text('["a"]', encoding="utf-8")
        committed.write_text('["a"]', encoding="utf-8")
        args = ["shipped", str(candidate), str(committed)]
        self.assertEqual(check.main(args), 0)
        committed.write_text('[]', encoding="utf-8")
        self.assertNotEqual(check.main(args), 0)

    def test_members_share_workspace_version(self):
        self.assertIsNone(check.manifest(self.root))
        path = self.root / "crates/greennode-sdk/Cargo.toml"
        path.write_text('[package]\nname = "greennode-sdk"\nversion = "0.2.0"\n', encoding="utf-8")
        with self.assertRaises(ValueError):
            check.manifest(self.root)


    def test_sdk_pin_and_path(self):
        path = self.root / "crates/greennode/Cargo.toml"
        base = path.read_text(encoding="utf-8")
        path.write_text(base + '\n[dependencies]\ngreennode-sdk = { path = "../greennode-sdk", version = "=0.1.0" }\n', encoding="utf-8")
        self.assertIsNone(check.manifest(self.root, tag="v0.1.0", release=True))
        path.write_text(base + '\n[dependencies]\ngreennode-sdk = { path = "../greennode-sdk", version = "0.1.0" }\n', encoding="utf-8")
        with self.assertRaises(ValueError):
            check.manifest(self.root)

    def test_tag_matches_workspace(self):
        self.assertIsNone(check.manifest(self.root, tag="v0.1.0"))
        with self.assertRaises(ValueError):
            check.manifest(self.root, tag="v0.2.0")

    def test_release_requires_sdk_dependency(self):
        self.assertIsNone(check.manifest(self.root))
        with self.assertRaises(ValueError):
            check.manifest(self.root, release=True)


    def test_shipped_equality_and_first_release(self):
        candidate = self.root / "candidate.json"
        committed = self.root / "committed.json"
        candidate.write_text('["a", "b"]', encoding="utf-8")
        committed.write_text('["a", "b"]', encoding="utf-8")
        self.assertIsNone(check.shipped(candidate, committed))
        candidate.write_text('["a"]', encoding="utf-8")
        with self.assertRaises(ValueError):
            check.shipped(candidate, committed)

    def test_shipped_requires_identical_bytes(self):
        candidate = self.root / "candidate.json"
        committed = self.root / "committed.json"
        candidate.write_text('["a"]\n', encoding="utf-8")
        committed.write_text('["a"]', encoding="utf-8")
        with self.assertRaises(ValueError):
            check.shipped(candidate, committed)
        committed.write_text('["a"]\n', encoding="utf-8")
        self.assertIsNone(check.shipped(candidate, committed))

    def test_previous_shipped_is_subset(self):
        candidate = self.root / "candidate.json"
        committed = self.root / "committed.json"
        previous = self.root / "previous.json"
        candidate.write_text('["a", "b"]', encoding="utf-8")
        committed.write_text('["a", "b"]', encoding="utf-8")
        previous.write_text('["a"]', encoding="utf-8")
        self.assertIsNone(check.shipped(candidate, committed, previous))
        previous.write_text('["c"]', encoding="utf-8")
        with self.assertRaises(ValueError):
            check.shipped(candidate, committed, previous)


class Receipt(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.stage = self.root / "stage"
        self.stage.mkdir()
        self.target = self.root / "receipt.json"
        self.make_pair()

    def archive(self, name, manifest, lock, extra=None):
        path = self.stage / f"{name}-0.1.0.crate"
        with tarfile.open(path, "w:gz") as archive:
            entries = {f"{name}-0.1.0/Cargo.toml": manifest,
                       f"{name}-0.1.0/Cargo.lock": lock}
            if extra:
                entries[extra] = "bad"
            for filename, content in entries.items():
                data = content.encode()
                info = tarfile.TarInfo(filename)
                info.size = len(data)
                archive.addfile(info, io.BytesIO(data))
        return hashlib.sha256(path.read_bytes()).hexdigest()

    def make_pair(self, version="0.1.0", requirement="=0.1.0", path="", checksum=None, extra=None):
        sdk = self.archive("greennode-sdk", f'[package]\nname="greennode-sdk"\nversion="{version}"\n', '')
        checksum = checksum if checksum is not None else sdk
        tool = self.archive("greennode", '[package]\nname="greennode"\nversion="0.1.0"\n'
                            f'[dependencies]\ngreennode-sdk = {{ version = "{requirement}"{path} }}\n',
                            f'[[package]]\nname="greennode-sdk"\nversion="0.1.0"\nchecksum="{checksum}"\n', extra)
        return sdk, tool

    def generate(self):
        check.receipt("v0.1.0", self.stage, self.target, "a" * 40, "1.100.0")

    def check_hostile(self):
        with self.assertRaises(ValueError):
            self.generate()
        self.assertFalse(self.target.exists())
        self.make_pair()
        self.generate()
        self.assertTrue(self.target.is_file())

    def test_receipt_error_names_rule(self):
        self.make_pair(checksum="0" * 64)
        errors = io.StringIO()
        with contextlib.redirect_stderr(errors):
            result = check.main(["receipt", "v0.1.0", str(self.stage), str(self.target),
                                 "--commit", "a" * 40, "--toolchain", "1.100.0"])
        self.assertNotEqual(result, 0)
        self.assertEqual(errors.getvalue(), "greennode: invalid SDK lockfile checksum\n")

    def test_receipt_command(self):
        args = ["receipt", "v0.1.0", str(self.stage), str(self.target),
                "--commit", "a" * 40, "--toolchain", "1.100.0"]
        self.assertEqual(check.main(args), 0)
        self.assertNotEqual(check.main(args), 0)

    def test_valid_pair_exact_receipt(self):
        sdk, tool = self.make_pair()
        self.generate()
        self.assertTrue(self.target.exists())
        expected = {"tag": "v0.1.0", "commit": "a" * 40, "toolchain": "1.100.0", "crates": [
            {"name": "greennode-sdk", "version": "0.1.0", "filename": "greennode-sdk-0.1.0.crate", "sha256": sdk},
            {"name": "greennode", "version": "0.1.0", "filename": "greennode-0.1.0.crate", "sha256": tool}]}
        self.assertEqual(self.target.read_text(encoding="utf-8"), json.dumps(expected, indent=2) + "\n")

    def test_wrong_version(self):
        self.make_pair(version="0.2.0")
        self.check_hostile()

    def test_path_dependency(self):
        self.make_pair(path=', path = "../greennode-sdk"')
        self.check_hostile()

    def test_loose_requirement(self):
        self.make_pair(requirement="0.1.0")
        self.check_hostile()

    def test_checksum_mismatch(self):
        self.make_pair(checksum="0" * 64)
        self.check_hostile()

    def test_extra_staged_file(self):
        extra = self.stage / "extra"
        extra.write_text("bad", encoding="utf-8")
        with self.assertRaises(ValueError):
            self.generate()
        extra.unlink()
        self.generate()
        self.assertTrue(self.target.is_file())

    def test_parent_archive_member(self):
        self.make_pair(extra="greennode-0.1.0/../bad")
        self.check_hostile()

    def test_existing_receipt(self):
        self.target.write_text("preserve", encoding="utf-8")
        with self.assertRaises(ValueError):
            self.generate()
        self.assertEqual(self.target.read_text(encoding="utf-8"), "preserve")
        self.target.unlink()
        self.generate()
        self.assertTrue(self.target.is_file())


class State(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.receipt = self.root / "receipt.json"
        self.records = [{"name": name, "version": "0.1.0", "filename": f"{name}-0.1.0.crate",
                         "sha256": "a" * 64} for name in ("greennode-sdk", "greennode")]
        self.data = {"tag": "v0.1.0", "commit": "a" * 40, "toolchain": "1.100.0", "crates": self.records}
        self.save()
        self.responses = {}
        self.sleeps = []
        self.absent()

    def save(self):
        self.receipt.write_text(json.dumps(self.data), encoding="utf-8")

    def absent(self):
        for record in self.records:
            name = record["name"]
            self.responses[f"https://crates.io/api/v1/crates/{name}/0.1.0"] = (404, b"")
            self.responses[f"https://crates.io/api/v1/crates/{name}"] = (404, b"")
            self.responses[f"https://index.crates.io/gr/ee/{name}"] = (404, b"")

    def approved(self, name):
        api = {"version": {"crate": name, "num": "0.1.0", "checksum": "a" * 64, "yanked": False}}
        index = {"name": name, "vers": "0.1.0", "cksum": "a" * 64, "yanked": False}
        self.responses[f"https://crates.io/api/v1/crates/{name}/0.1.0"] = (200, json.dumps(api).encode())
        self.responses[f"https://index.crates.io/gr/ee/{name}"] = (200, json.dumps(index).encode())

    def fetch(self, url):
        result = self.responses[url]
        if isinstance(result, Exception):
            raise result
        return result

    def run_state(self, **kwargs):
        output = io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            result = check.state(self.receipt, fetch=self.fetch, sleep=self.sleeps.append, **kwargs)
        return result, output.getvalue()

    def check_stop(self):
        result, output = self.run_state()
        self.assertNotEqual(result, 0)
        self.assertIn("stop", output)
        self.absent()
        self.assertEqual(self.run_state()[0], 0)

    def test_state_command(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            result = check.main(["state", str(self.receipt), "--first"], fetch=self.fetch, sleep=self.sleeps.append)
        self.assertEqual(result, 0)
        self.assertEqual(output.getvalue(), "greennode-sdk: absent\ngreennode: absent\n")
        self.assertNotEqual(check.main(["state", str(self.receipt), "--bad"], fetch=self.fetch), 0)

    def test_absent(self):
        self.assertEqual(self.run_state(), (0, "greennode-sdk: absent\ngreennode: absent\n"))
        self.responses["https://crates.io/api/v1/crates/greennode/0.1.0"] = (500, b"secret")
        self.assertNotEqual(self.run_state()[0], 0)

    def test_approved(self):
        for name in ("greennode-sdk", "greennode"):
            self.approved(name)
        self.assertEqual(self.run_state(), (0, "greennode-sdk: present-and-approved\ngreennode: present-and-approved\n"))
        self.responses["https://index.crates.io/gr/ee/greennode"] = (500, b"")
        self.assertNotEqual(self.run_state()[0], 0)

    def test_yanked(self):
        self.approved("greennode")
        key = "https://index.crates.io/gr/ee/greennode"
        value = json.loads(self.responses[key][1]); value["yanked"] = True
        self.responses[key] = (200, json.dumps(value).encode())
        self.check_stop()

    def test_checksum_mismatch(self):
        self.approved("greennode")
        key = "https://index.crates.io/gr/ee/greennode"
        value = json.loads(self.responses[key][1]); value["cksum"] = "b" * 64
        self.responses[key] = (200, json.dumps(value).encode())
        self.check_stop()

    def test_disagreement(self):
        self.approved("greennode")
        self.responses["https://index.crates.io/gr/ee/greennode"] = (404, b"")
        self.check_stop()

    def test_timeout(self):
        self.responses["https://crates.io/api/v1/crates/greennode/0.1.0"] = TimeoutError()
        self.check_stop()

    def test_malformed_json(self):
        self.responses["https://index.crates.io/gr/ee/greennode"] = (200, b"sensitive")
        self.check_stop()

    def test_first_requires_unregistered_names(self):
        self.responses["https://crates.io/api/v1/crates/greennode"] = (200, b"{}")
        self.assertNotEqual(self.run_state(first=True)[0], 0)
        self.absent()
        self.assertEqual(self.run_state(first=True)[0], 0)

    def test_sdk_only_recovery(self):
        self.approved("greennode-sdk")
        self.assertEqual(self.run_state(), (0, "greennode-sdk: present-and-approved\ngreennode: absent\n"))
        self.responses["https://index.crates.io/gr/ee/greennode-sdk"] = (404, b"")
        self.assertNotEqual(self.run_state()[0], 0)

    def test_wait_yanked_stops(self):
        self.approved("greennode-sdk")
        key = "https://index.crates.io/gr/ee/greennode-sdk"
        value = json.loads(self.responses[key][1]); value["yanked"] = True
        self.responses[key] = (200, json.dumps(value).encode())
        result, output = self.run_state(wait="greennode-sdk")
        self.assertNotEqual(result, 0)
        self.assertIn("greennode-sdk: stop", output)
        self.assertEqual(self.sleeps, [])
        self.approved("greennode-sdk")
        self.assertEqual(self.run_state(wait="greennode-sdk")[0], 0)

    def test_malformed_other_index_version(self):
        key = "https://index.crates.io/gr/ee/greennode"
        self.responses[key] = (200, b'{"name":"greennode","vers":"0.0.1"}')
        self.check_stop()

    def test_empty_index_is_malformed(self):
        self.responses["https://index.crates.io/gr/ee/greennode"] = (200, b"")
        self.check_stop()

    def test_wait_bound(self):
        result, output = self.run_state(wait="greennode-sdk")
        self.assertNotEqual(result, 0)
        self.assertIn("greennode-sdk: unknown version=0.1.0 sha256=" + "a" * 64, output)
        self.assertGreater(len(self.sleeps), 0)
        self.assertLess(len(self.sleeps), 100)
        self.approved("greennode-sdk")
        self.assertEqual(self.run_state(wait="greennode-sdk")[0], 0)

    def test_receipt_strict(self):
        self.data["extra"] = "bad"
        self.save()
        self.assertNotEqual(self.run_state()[0], 0)
        del self.data["extra"]
        self.save()
        self.assertEqual(self.run_state()[0], 0)


if __name__ == "__main__":
    unittest.main()
