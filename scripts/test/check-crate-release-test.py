import contextlib
import gzip
import hashlib
import http.server
import threading
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
BUILTINS = ["greennode", "greennode --help", "greennode --version", "greennode -h",
            "greennode -v", "greennode help", "greennode version"]


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
        candidate.write_text(json.dumps(BUILTINS), encoding="utf-8")
        committed.write_text(json.dumps(BUILTINS), encoding="utf-8")
        args = ["shipped", str(candidate), str(committed), "--first"]
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

    def test_renamed_sdk_pin(self):
        path = self.root / "crates/greennode/Cargo.toml"
        base = path.read_text(encoding="utf-8")
        path.write_text(base + '\n[dependencies]\nother = { package = "greennode-sdk", path = "../greennode-sdk", version = "0.1.0" }\n', encoding="utf-8")
        with self.assertRaises(ValueError):
            check.manifest(self.root)
        path.write_text(base + '\n[dependencies]\nother = { package = "greennode-sdk", path = "../greennode-sdk", version = "=0.1.0" }\n', encoding="utf-8")
        self.assertIsNone(check.manifest(self.root, release=True))

    def test_build_and_dev_dependency_pins(self):
        path = self.root / "crates/greennode/Cargo.toml"
        base = path.read_text(encoding="utf-8")
        for section in ("build-dependencies", "dev-dependencies", "target.'cfg(unix)'.dev-dependencies"):
            with self.subTest(section=section):
                prefix = base + f'\n[{section}]\ngreennode-sdk = {{ path = "../greennode-sdk", version = '
                path.write_text(prefix + '"0.1.0" }\n', encoding="utf-8")
                with self.assertRaises(ValueError):
                    check.manifest(self.root)
                path.write_text(prefix + '"=0.1.0" }\n', encoding="utf-8")
                self.assertIsNone(check.manifest(self.root))

    def test_target_sdk_pin(self):
        path = self.root / "crates/greennode/Cargo.toml"
        base = path.read_text(encoding="utf-8")
        path.write_text(base + '\n[target.\'cfg(unix)\'.build-dependencies]\ngreennode-sdk = { path = "../greennode-sdk", version = "0.1.0" }\n', encoding="utf-8")
        with self.assertRaises(ValueError):
            check.manifest(self.root)
        path.write_text(base + '\n[target.\'cfg(unix)\'.build-dependencies]\ngreennode-sdk = { path = "../greennode-sdk", version = "=0.1.0" }\n', encoding="utf-8")
        self.assertIsNone(check.manifest(self.root))

    def test_manifest_path_guard(self):
        path = self.root / "crates/greennode/Cargo.toml"
        base = path.read_text(encoding="utf-8")
        dependency = '\n[dependencies]\ngreennode-sdk = { path = "%s", version = "=0.1.0" }\n'
        path.write_text(base + dependency % "../wrong", encoding="utf-8")
        with self.assertRaises(ValueError):
            check.manifest(self.root)
        path.write_text(base + dependency % "../greennode-sdk", encoding="utf-8")
        self.assertIsNone(check.manifest(self.root, release=True))

    def test_member_sdk_workspace_inheritance_refused(self):
        path = self.root / "crates/greennode/Cargo.toml"
        base = path.read_text(encoding="utf-8")
        entry = '\n[dependencies]\ngreennode-sdk = { path = "../greennode-sdk", version = "=0.1.0"%s }\n'
        path.write_text(base + entry % ', workspace = true', encoding="utf-8")
        with self.assertRaises(ValueError):
            check.manifest(self.root)
        path.write_text(base + entry % '', encoding="utf-8")
        self.assertIsNone(check.manifest(self.root, release=True))

    def test_workspace_sdk_declaration_refused(self):
        path = self.root / "Cargo.toml"
        base = path.read_text(encoding="utf-8")
        path.write_text(base + '\n[workspace.dependencies]\nclient = { package = "greennode-sdk", path = "crates/greennode-sdk", version = "=0.1.0" }\n', encoding="utf-8")
        with self.assertRaises(ValueError):
            check.manifest(self.root)
        path.write_text(base, encoding="utf-8")
        self.assertIsNone(check.manifest(self.root))

    def test_release_requires_normal_dependency(self):
        path = self.root / "crates/greennode/Cargo.toml"
        base = path.read_text(encoding="utf-8")
        entry = '\ngreennode-sdk = { path = "../greennode-sdk", version = "=0.1.0" }\n'
        path.write_text(base + '\n[dev-dependencies]' + entry, encoding="utf-8")
        with self.assertRaises(ValueError):
            check.manifest(self.root, release=True)
        path.write_text(base + '\n[dependencies]' + entry, encoding="utf-8")
        self.assertIsNone(check.manifest(self.root, release=True))

    def test_tag_matches_workspace(self):
        self.assertIsNone(check.manifest(self.root, tag="v0.1.0"))
        with self.assertRaises(ValueError):
            check.manifest(self.root, tag="v0.2.0")

    def test_release_requires_sdk_dependency(self):
        self.assertIsNone(check.manifest(self.root))
        with self.assertRaises(ValueError):
            check.manifest(self.root, release=True)


    def test_shipped_requires_builtin_paths(self):
        path = self.root / "shipped.json"
        path.write_text('[]', encoding="utf-8")
        with self.assertRaises(ValueError):
            check.shipped(path, path)
        paths = ["greennode", "greennode --help", "greennode --version", "greennode -h",
                 "greennode -v", "greennode help", "greennode version"]
        path.write_text(json.dumps(paths), encoding="utf-8")
        self.assertIsNone(check.shipped(path, path, first=True))

    def test_shipped_requires_each_builtin(self):
        path = self.root / "shipped.json"
        path.write_text(json.dumps(BUILTINS[:-1]), encoding="utf-8")
        with self.assertRaises(ValueError):
            check.shipped(path, path, first=True)
        path.write_text(json.dumps(BUILTINS), encoding="utf-8")
        self.assertIsNone(check.shipped(path, path, first=True))

    def test_operation_requires_service_parent(self):
        path = self.root / "shipped.json"
        path.write_text(json.dumps(sorted(BUILTINS + ["greennode compute list"])), encoding="utf-8")
        with self.assertRaises(ValueError):
            check.shipped(path, path, first=True)
        path.write_text(json.dumps(sorted(BUILTINS + ["greennode compute", "greennode compute list"])), encoding="utf-8")
        self.assertIsNone(check.shipped(path, path, first=True))

    def test_shipped_rejects_hidden_command_path(self):
        path = self.root / "shipped.json"
        path.write_text(json.dumps(sorted(BUILTINS + ["greennode gen-docs output"])), encoding="utf-8")
        with self.assertRaises(ValueError):
            check.shipped(path, path, first=True)
        path.write_text(json.dumps(sorted(BUILTINS + ["greennode compute", "greennode compute list"])), encoding="utf-8")
        self.assertIsNone(check.shipped(path, path, first=True))

    def test_shipped_rejects_invalid_path_shape(self):
        path = self.root / "shipped.json"
        path.write_text(json.dumps(sorted(BUILTINS + ["greennode gen-docs"])), encoding="utf-8")
        with self.assertRaises(ValueError):
            check.shipped(path, path, first=True)
        path.write_text(json.dumps(sorted(BUILTINS + ["greennode compute", "greennode compute list"])), encoding="utf-8")
        self.assertIsNone(check.shipped(path, path, first=True))

    def test_shipped_rejects_duplicate_and_unsorted(self):
        path = self.root / "shipped.json"
        for paths in (sorted(BUILTINS + ["greennode version"]), list(reversed(BUILTINS))):
            path.write_text(json.dumps(paths), encoding="utf-8")
            with self.assertRaises(ValueError):
                check.shipped(path, path, first=True)
        path.write_text(json.dumps(BUILTINS), encoding="utf-8")
        self.assertIsNone(check.shipped(path, path, first=True))

    def test_shipped_rejects_unsorted(self):
        path = self.root / "shipped.json"
        path.write_text(json.dumps(list(reversed(BUILTINS))), encoding="utf-8")
        with self.assertRaises(ValueError):
            check.shipped(path, path, first=True)
        path.write_text(json.dumps(BUILTINS), encoding="utf-8")
        self.assertIsNone(check.shipped(path, path, first=True))

    def test_shipped_requires_explicit_first(self):
        path = self.root / "shipped.json"
        path.write_text(json.dumps(BUILTINS), encoding="utf-8")
        with self.assertRaises(ValueError):
            check.shipped(path, path)
        paths = ["greennode", "greennode --help", "greennode --version", "greennode -h",
                 "greennode -v", "greennode help", "greennode version"]
        path.write_text(json.dumps(paths), encoding="utf-8")
        self.assertEqual(check.main(["shipped", str(path), str(path), "--first"]), 0)

    def test_shipped_equality_and_first_release(self):
        candidate = self.root / "candidate.json"
        committed = self.root / "committed.json"
        candidate.write_text(json.dumps(BUILTINS), encoding="utf-8")
        committed.write_text(json.dumps(BUILTINS), encoding="utf-8")
        self.assertIsNone(check.shipped(candidate, committed, first=True))
        candidate.write_text(json.dumps(sorted(BUILTINS + ["greennode test", "greennode test read"])), encoding="utf-8")
        with self.assertRaises(ValueError):
            check.shipped(candidate, committed, first=True)

    def test_shipped_requires_identical_bytes(self):
        candidate = self.root / "candidate.json"
        committed = self.root / "committed.json"
        candidate.write_text(json.dumps(BUILTINS) + '\n', encoding="utf-8")
        committed.write_text(json.dumps(BUILTINS), encoding="utf-8")
        with self.assertRaises(ValueError):
            check.shipped(candidate, committed, first=True)
        committed.write_text(json.dumps(BUILTINS) + '\n', encoding="utf-8")
        self.assertIsNone(check.shipped(candidate, committed, first=True))

    def test_previous_shipped_is_subset(self):
        candidate = self.root / "candidate.json"
        committed = self.root / "committed.json"
        previous = self.root / "previous.json"
        candidate.write_text(json.dumps(sorted(BUILTINS + ["greennode test", "greennode test read"])), encoding="utf-8")
        committed.write_text(json.dumps(sorted(BUILTINS + ["greennode test", "greennode test read"])), encoding="utf-8")
        previous.write_text(json.dumps(BUILTINS), encoding="utf-8")
        self.assertIsNone(check.shipped(candidate, committed, previous))
        previous.write_text(json.dumps(sorted(BUILTINS + ["greennode test", "greennode test write"])), encoding="utf-8")
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

    def make_pair(self, version="0.1.0", requirement="=0.1.0", path="", checksum=None, extra=None, source=True):
        sdk = self.archive("greennode-sdk", f'[package]\nname="greennode-sdk"\nversion="{version}"\n', '')
        checksum = checksum if checksum is not None else sdk
        tool = self.archive("greennode", '[package]\nname="greennode"\nversion="0.1.0"\n'
                            f'[dependencies]\ngreennode-sdk = {{ version = "{requirement}"{path} }}\n',
                            f'[[package]]\nname="greennode-sdk"\nversion="0.1.0"\nchecksum="{checksum}"\n'
                            + ('source="registry+https://github.com/rust-lang/crates.io-index"\n' if source else ''), extra)
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
        self.assertEqual(errors.getvalue(), "greennode: invalid SDK lockfile checksum or source\n")

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

    def test_normalized_archive_alias_is_rejected(self):
        self.archive("greennode-sdk", '[package]\nname="greennode-sdk"\nversion="0.1.0"\n', '',
                     "greennode-sdk-0.1.0//Cargo.toml")
        data = (self.stage / "greennode-sdk-0.1.0.crate").read_bytes()
        with self.assertRaises(ValueError):
            check.archive_data("greennode-sdk", "0.1.0", data)
        self.make_pair()
        data = (self.stage / "greennode-sdk-0.1.0.crate").read_bytes()
        self.assertIsNotNone(check.archive_data("greennode-sdk", "0.1.0", data))

    def test_inflated_size_bound(self):
        data = (self.stage / "greennode-sdk-0.1.0.crate").read_bytes()
        oversized = gzip.compress(gzip.decompress(data) + bytes(50 * 1024 * 1024))
        with self.assertRaises(ValueError):
            check.archive_data("greennode-sdk", "0.1.0", oversized)
        self.assertIsNotNone(check.archive_data("greennode-sdk", "0.1.0", data))

    def test_gzip_trailer_required(self):
        data = (self.stage / "greennode-sdk-0.1.0.crate").read_bytes()
        with self.assertRaises(ValueError):
            check.archive_data("greennode-sdk", "0.1.0", data[:-4])
        self.assertIsNotNone(check.archive_data("greennode-sdk", "0.1.0", data))

    def test_tar_two_end_blocks_required(self):
        data = (self.stage / "greennode-sdk-0.1.0.crate").read_bytes()
        raw = gzip.decompress(data)
        with tarfile.open(fileobj=io.BytesIO(raw), mode="r:") as archive:
            members = archive.getmembers()
        last = members[-1]
        end = last.offset_data + ((last.size + 511) // 512) * 512
        with self.assertRaises(ValueError):
            check.archive_data("greennode-sdk", "0.1.0", gzip.compress(raw[:end] + bytes(512)))
        self.assertIsNotNone(check.archive_data("greennode-sdk", "0.1.0", data))

    def test_archive_link_rejected(self):
        data = (self.stage / "greennode-sdk-0.1.0.crate").read_bytes()
        raw = gzip.decompress(data)
        info = tarfile.TarInfo("greennode-sdk-0.1.0/link")
        info.type = tarfile.SYMTYPE
        info.linkname = "Cargo.toml"
        malicious = gzip.compress(info.tobuf() + raw)
        with self.assertRaises(ValueError):
            check.archive_data("greennode-sdk", "0.1.0", malicious)
        self.assertIsNotNone(check.archive_data("greennode-sdk", "0.1.0", data))

    def test_extended_headers_rejected(self):
        data = (self.stage / "greennode-sdk-0.1.0.crate").read_bytes()
        raw = gzip.decompress(data)
        info = tarfile.TarInfo("extended")
        info.type = tarfile.XHDTYPE
        payload = b"13 comment=x\n"
        info.size = len(payload)
        extended = info.tobuf() + payload + bytes(512 - len(payload)) + raw
        with self.assertRaises(ValueError):
            check.archive_data("greennode-sdk", "0.1.0", gzip.compress(extended))
        self.assertIsNotNone(check.archive_data("greennode-sdk", "0.1.0", data))

    def test_lockfile_requires_registry_source(self):
        self.make_pair(source=False)
        with self.assertRaises(ValueError):
            self.generate()
        self.make_pair(source=True)
        self.generate()
        self.assertTrue(self.target.exists())

    def test_packaged_target_alias_refused(self):
        sdk = hashlib.sha256((self.stage / "greennode-sdk-0.1.0.crate").read_bytes()).hexdigest()
        base = ('[package]\nname="greennode"\nversion="0.1.0"\n'
                '[dependencies]\ngreennode-sdk = { version = "=0.1.0" }\n')
        lock = (f'[[package]]\nname="greennode-sdk"\nversion="0.1.0"\nchecksum="{sdk}"\n'
                'source="registry+https://github.com/rust-lang/crates.io-index"\n')
        hostile = base + '[target.\'cfg(unix)\'.dev-dependencies]\nalias = { package = "greennode-sdk", version = "0.1.0" }\n'
        self.archive("greennode", hostile, lock)
        self.check_hostile()

    def test_file_ancestor_conflict(self):
        data = (self.stage / "greennode-sdk-0.1.0.crate").read_bytes()
        raw = gzip.decompress(data)
        file = tarfile.TarInfo("greennode-sdk-0.1.0/node")
        child = tarfile.TarInfo("greennode-sdk-0.1.0/node/child")
        for additions in (file.tobuf() + child.tobuf(), child.tobuf() + file.tobuf()):
            with self.subTest(order=additions[:100]):
                with self.assertRaises(ValueError):
                    check.archive_data("greennode-sdk", "0.1.0", gzip.compress(additions + raw))
        self.assertIsNotNone(check.archive_data("greennode-sdk", "0.1.0", data))

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
            body = json.dumps({"errors": [{"detail": f"crate `{name}` does not exist"}]}).encode()
            index = f"<Error><Code>NoSuchKey</Code><Key>gr/ee/{name}</Key></Error>".encode()
            self.responses[f"https://crates.io/api/v1/crates/{name}/0.1.0"] = (404, body)
            self.responses[f"https://crates.io/api/v1/crates/{name}"] = (404, body)
            self.responses[f"https://index.crates.io/gr/ee/{name}"] = (404, index)

    def approved(self, name):
        api = {"version": {"crate": name, "num": "0.1.0", "checksum": "a" * 64, "yanked": False}}
        index = {"name": name, "vers": "0.1.0", "cksum": "a" * 64, "yanked": False}
        self.responses[f"https://crates.io/api/v1/crates/{name}/0.1.0"] = (200, json.dumps(api).encode())
        self.responses[f"https://index.crates.io/gr/ee/{name}"] = (200, json.dumps(index).encode() + b"\n")

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

    def test_transport_exception_is_redacted(self):
        self.responses["https://crates.io/api/v1/crates/greennode/0.1.0"] = RuntimeError("private-canary")
        result, output = self.run_state()
        self.assertNotEqual(result, 0)
        self.assertIn("greennode: stop", output)
        self.assertNotIn("private-canary", output)
        self.absent()
        self.assertEqual(self.run_state()[0], 0)

    def test_index_404_requires_real_xml(self):
        key = "https://index.crates.io/gr/ee/greennode"
        self.responses[key] = (404, b'<Error><!--<Code>NoSuchKey</Code><Key>gr/ee/greennode</Key>--></Error>')
        self.check_stop()

    def test_index_404_rejects_doctype(self):
        key = "https://index.crates.io/gr/ee/greennode"
        self.responses[key] = (404, b'<!DOCTYPE Error><Error><Code>NoSuchKey</Code><Key>gr/ee/greennode</Key></Error>')
        self.check_stop()

    def test_api_exact_version_guard(self):
        self.check_api_guard("num", "0.2.0")

    def test_generic_api_404_stops(self):
        self.responses["https://crates.io/api/v1/crates/greennode/0.1.0"] = (404, b"not found")
        self.check_stop()

    def test_incomplete_index_stops(self):
        self.approved("greennode")
        self.responses["https://crates.io/api/v1/crates/greennode/0.1.0"] = (
            404, b'{"errors":[{"detail":"crate `greennode` does not exist"}]}')
        key = "https://index.crates.io/gr/ee/greennode"
        old = json.loads(self.responses[key][1]); old["vers"] = "0.0.1"
        self.responses[key] = (200, json.dumps(old).encode())
        self.check_stop()

    def test_bad_index_even_when_api_absent(self):
        self.approved("greennode")
        self.responses["https://crates.io/api/v1/crates/greennode/0.1.0"] = (
            404, b'{"errors":[{"detail":"crate `greennode` does not exist"}]}')
        key = "https://index.crates.io/gr/ee/greennode"
        row = json.loads(self.responses[key][1]); row["yanked"] = True
        self.responses[key] = (200, json.dumps(row).encode() + b"\n")
        self.assertEqual(check.registry_state(self.records[1], self.fetch), "stop")

    def test_first_rejects_existing_index(self):
        self.approved("greennode")
        self.responses["https://crates.io/api/v1/crates/greennode/0.1.0"] = (
            404, b'{"errors":[{"detail":"crate `greennode` does not exist"}]}')
        key = "https://index.crates.io/gr/ee/greennode"
        row = json.loads(self.responses[key][1]); row["vers"] = "0.0.1"
        self.responses[key] = (200, json.dumps(row).encode() + b"\n")
        self.assertNotEqual(self.run_state(first=True)[0], 0)
        self.absent()
        self.assertEqual(self.run_state(first=True)[0], 0)

    def test_wait_server_error_unknown(self):
        self.responses["https://crates.io/api/v1/crates/greennode-sdk/0.1.0"] = (503, b"private")
        result, output = self.run_state(wait="greennode-sdk")
        self.assertNotEqual(result, 0)
        self.assertIn("greennode-sdk: unknown version=0.1.0 sha256=" + "a" * 64, output)
        self.assertNotIn("private", output)

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

    def test_api_identity_guard(self):
        self.check_api_guard("crate", "other")

    def test_api_checksum_guard(self):
        self.check_api_guard("checksum", "b" * 64)

    def test_api_yank_guard(self):
        self.check_api_guard("yanked", True)

    def check_api_guard(self, field, value):
        self.approved("greennode")
        key = "https://crates.io/api/v1/crates/greennode/0.1.0"
        payload = json.loads(self.responses[key][1])
        payload["version"][field] = value
        self.responses[key] = (200, json.dumps(payload).encode())
        self.assertEqual(check.registry_state(self.records[1], self.fetch), "stop")
        self.approved("greennode")
        self.assertEqual(check.registry_state(self.records[1], self.fetch), "present-and-approved")

    def test_index_status_guard(self):
        self.responses["https://index.crates.io/gr/ee/greennode"] = (403, b"")
        self.assertEqual(check.registry_state(self.records[1], self.fetch), "stop")
        self.absent()
        self.assertEqual(check.registry_state(self.records[1], self.fetch), "absent")

    def test_yanked(self):
        self.approved("greennode")
        key = "https://index.crates.io/gr/ee/greennode"
        value = json.loads(self.responses[key][1]); value["yanked"] = True
        self.responses[key] = (200, json.dumps(value).encode() + b"\n")
        self.check_stop()

    def test_checksum_mismatch(self):
        self.approved("greennode")
        key = "https://index.crates.io/gr/ee/greennode"
        value = json.loads(self.responses[key][1]); value["cksum"] = "b" * 64
        self.responses[key] = (200, json.dumps(value).encode() + b"\n")
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

    def test_bad_index_remains_terminal_with_api_5xx(self):
        self.approved("greennode")
        self.responses["https://crates.io/api/v1/crates/greennode/0.1.0"] = (503, b"")
        key = "https://index.crates.io/gr/ee/greennode"
        good = self.responses[key]
        row = json.loads(good[1]); row["yanked"] = True
        self.responses[key] = (200, json.dumps(row).encode() + b"\n")
        self.assertEqual(check.registry_state(self.records[1], self.fetch), "stop")
        self.responses[key] = good
        self.assertEqual(check.registry_state(self.records[1], self.fetch), "transient")

    def test_yank_cannot_be_cleared_by_retry(self):
        self.approved("greennode-sdk")
        key = "https://index.crates.io/gr/ee/greennode-sdk"
        good = self.responses[key]
        row = json.loads(good[1]); row["yanked"] = True
        bad = (200, json.dumps(row).encode() + b"\n")
        calls = []
        def fetch(url):
            if url == key:
                calls.append(url)
                return bad if len(calls) == 1 else good
            return self.fetch(url)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            result = check.state(self.receipt, wait="greennode-sdk", fetch=fetch, sleep=self.sleeps.append)
        self.assertNotEqual(result, 0)
        self.assertIn("greennode-sdk: unknown", output.getvalue())
        self.assertEqual(len(calls), 1)
        self.assertEqual(self.sleeps, [])
        self.assertEqual(self.run_state(wait="greennode-sdk")[0], 0)

    def test_wait_yanked_unknown(self):
        self.approved("greennode-sdk")
        key = "https://index.crates.io/gr/ee/greennode-sdk"
        value = json.loads(self.responses[key][1]); value["yanked"] = True
        self.responses[key] = (200, json.dumps(value).encode() + b"\n")
        result, output = self.run_state(wait="greennode-sdk")
        self.assertNotEqual(result, 0)
        self.assertIn("greennode-sdk: unknown", output)
        self.assertEqual(len(self.sleeps), 0)
        self.approved("greennode-sdk")
        self.assertEqual(self.run_state(wait="greennode-sdk")[0], 0)

    def test_malformed_other_index_version(self):
        key = "https://index.crates.io/gr/ee/greennode"
        self.responses[key] = (200, b'{"name":"greennode","vers":"0.0.1"}')
        self.check_stop()

    def test_empty_index_is_malformed(self):
        self.responses["https://index.crates.io/gr/ee/greennode"] = (200, b"")
        self.check_stop()

    def test_wait_deadline(self):
        ticks = [0]
        calls = []
        def fetch(url):
            calls.append(url)
            ticks[0] += 40
            return self.fetch(url)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            result = check.state(self.receipt, wait="greennode-sdk", fetch=fetch,
                                 sleep=self.sleeps.append, clock=lambda: ticks[0])
        self.assertNotEqual(result, 0)
        self.assertIn("greennode-sdk: unknown", output.getvalue())
        self.assertLessEqual(len([url for url in calls if "greennode-sdk" in url]), 2)

    def test_wait_absent_is_terminal(self):
        result, output = self.run_state(wait="greennode-sdk")
        self.assertNotEqual(result, 0)
        self.assertIn("greennode-sdk: unknown", output)
        self.assertEqual(self.sleeps, [])
        self.approved("greennode-sdk")
        self.assertEqual(self.run_state(wait="greennode-sdk")[0], 0)

    def test_wait_bound(self):
        self.responses["https://crates.io/api/v1/crates/greennode-sdk/0.1.0"] = (503, b"")
        result, output = self.run_state(wait="greennode-sdk")
        self.assertNotEqual(result, 0)
        self.assertIn("greennode-sdk: unknown version=0.1.0 sha256=" + "a" * 64, output)
        self.assertEqual(len(self.sleeps), 4)
        self.approved("greennode-sdk")
        self.assertEqual(self.run_state(wait="greennode-sdk")[0], 0)

    def test_receipt_strict(self):
        self.data["extra"] = "bad"
        self.save()
        self.assertNotEqual(self.run_state()[0], 0)
        del self.data["extra"]
        self.save()
        self.assertEqual(self.run_state()[0], 0)


class Transport(unittest.TestCase):
    def test_fetch_hard_deadline(self):
        release = threading.Event()
        finished = threading.Event()
        def stalled():
            release.wait(0.2)
            finished.set()
            return 200, b"ok"
        try:
            with self.assertRaises(TimeoutError):
                check.deadline_fetch(stalled, 0.01)
        finally:
            release.set()
            self.assertTrue(finished.wait(1))
        self.assertEqual(check.deadline_fetch(lambda: (200, b"ok"), 1), (200, b"ok"))

    def test_trickling_body_expires_deadline(self):
        ticks = [0]

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(200)
                self.send_header("Content-Length", "8")
                self.end_headers()
                try:
                    for byte in b"12345678":
                        ticks[0] += 1
                        self.wfile.write(bytes([byte]))
                        self.wfile.flush()
                except OSError:
                    pass

            def log_message(self, format, *args):
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        self.addCleanup(thread.join)
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        url = f"http://127.0.0.1:{server.server_port}/"
        with self.assertRaises(TimeoutError):
            check.fetch_url(url, allow_loopback=True, deadline=3, clock=lambda: ticks[0])
        self.assertEqual(check.fetch_url(url, allow_loopback=True), (200, b"12345678"))
        with tempfile.TemporaryDirectory() as temporary:
            receipt = Path(temporary) / "receipt.json"
            records = [{"name": name, "version": "0.1.0", "filename": f"{name}-0.1.0.crate",
                        "sha256": "a" * 64} for name in ("greennode-sdk", "greennode")]
            receipt.write_text(json.dumps({"tag": "v0.1.0", "commit": "a" * 40,
                                           "toolchain": "1.100.0", "crates": records}), encoding="utf-8")

            def fetch(target):
                if "greennode-sdk" in target:
                    ticks[0] = 0
                    return check.fetch_url(url, allow_loopback=True, deadline=3, clock=lambda: ticks[0])
                if "index.crates.io" in target:
                    return 404, b"<Code>NoSuchKey</Code><Key>gr/ee/greennode</Key>"
                return 404, b'{"errors":[{"detail":"crate `greennode` does not exist"}]}'

            for wait, expected in ((None, "stop"), ("greennode-sdk", "unknown")):
                output = io.StringIO()
                with contextlib.redirect_stdout(output):
                    result = check.state(receipt, wait=wait, fetch=fetch, sleep=lambda _: None)
                self.assertNotEqual(result, 0)
                self.assertIn(f"greennode-sdk: {expected}", output.getvalue())

    def test_loopback_404_framing_and_redirect(self):
        requests = []
        body = b'{"errors":[{"detail":"crate `greennode` does not exist"}]}'

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                requests.append(self.path)
                if self.path == "/redirect":
                    self.send_response(302)
                    self.send_header("Location", "/followed")
                    payload = b""
                else:
                    self.send_response(404)
                    payload = body if self.path != "/generic" else b"not found"
                    if self.path == "/index":
                        payload = b"<Error><Code>NoSuchKey</Code><Key>gr/ee/greennode</Key></Error>"
                self.send_header("Content-Length", str(len(payload) + (10 if self.path == "/truncated" else 0)))
                self.end_headers()
                self.wfile.write(payload)

            def log_message(self, format, *args):
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        self.addCleanup(thread.join)
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        base = f"http://127.0.0.1:{server.server_port}"
        try:
            status, response = check.fetch_url(base + "/valid", allow_loopback=True)
        except (TypeError, ValueError):
            self.fail("loopback fetch did not return the not-found body")
        self.assertEqual((status, response), (404, body))
        self.assertTrue(check.api_not_found(response, "greennode", "0.1.0"))
        status, response = check.fetch_url(base + "/index", allow_loopback=True)
        self.assertEqual(status, 404)
        self.assertTrue(check.index_not_found(response, "greennode"))
        status, response = check.fetch_url(base + "/generic", allow_loopback=True)
        self.assertFalse(check.index_not_found(response, "greennode"))
        self.assertEqual((status, response), (404, b"not found"))
        with self.assertRaises((OSError, ValueError)):
            check.fetch_url(base + "/truncated", allow_loopback=True)
        self.assertEqual(check.fetch_url(base + "/redirect", allow_loopback=True)[0], 302)
        self.assertNotIn("/followed", requests)


if __name__ == "__main__":
    unittest.main()
