# Usage: python3 -I scripts/test/check-crate-publish-credential-test.py
import types
import io
import json
import os
import tempfile
import unittest
from unittest.mock import patch
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "check-crate-publish-credential.py"


def load():
    if not SCRIPT.exists():
        return None
    module = types.ModuleType("credential_provider")
    exec(compile(SCRIPT.read_bytes(), str(SCRIPT), "exec"), module.__dict__)
    return module


class CredentialProvider(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.receipt = {
            "tag": "v0.1.0", "commit": "a" * 40, "toolchain": "1.100.0",
            "crates": [
                {"name": name, "version": "0.1.0", "filename": name + "-0.1.0.crate", "sha256": digest * 64}
                for name, digest in (("greennode-sdk", "a"), ("greennode", "b"))
            ],
        }
        self.path = self.root / "receipt.json"
        self.save()
        self.env = {"GREENNODE_RELEASE_RECEIPT": str(self.path), "CARGO_REGISTRY_TOKEN": "toy-canary"}
        self.request = {"v": 1, "kind": "get", "operation": "publish", "name": "greennode-sdk", "vers": "0.1.0", "cksum": "a" * 64, "registry": {"index-url": "sparse+https://index.crates.io/", "name": "crates-io"}}

    def save(self):
        self.path.write_text(json.dumps(self.receipt))

    def exchange(self, request=None):
        provider = load()
        output, errors = io.StringIO(), io.StringIO()
        if provider:
            provider.main(io.StringIO(json.dumps(request or self.request) + "\n"), output, errors, self.env)
        self.assertFalse("toy-canary" in errors.getvalue())
        lines = output.getvalue().splitlines()
        self.assertGreaterEqual(len(lines), 2)
        self.assertEqual(json.loads(lines[0]), {"v": [1]})
        return json.loads(lines[1])

    def accepted(self, request=None):
        response = self.exchange(request)
        self.assertTrue("Ok" in response)
        result = response["Ok"]
        self.assertTrue(result.get("token") == "toy-canary")
        self.assertEqual({key: value for key, value in result.items() if key != "token"},
                         {"kind": "get", "cache": "never", "operation_independent": False})

    def rejected(self, request):
        response = self.exchange(request)
        self.assertTrue("Err" in response)
        self.assertFalse("toy-canary" in json.dumps(response))

    def test_matching_publish_and_wrong_checksum(self):
        self.accepted()
        self.rejected(dict(self.request, cksum="b" * 64))

    def test_matching_publish_and_wrong_version(self):
        self.accepted()
        self.rejected(dict(self.request, vers="0.1.1"))

    def test_matching_publish_and_unknown_crate(self):
        self.accepted()
        self.rejected(dict(self.request, name="other"))

    def test_crates_io_and_other_registry(self):
        self.accepted()
        self.rejected(dict(self.request, registry={"index-url": "sparse+https://example.org/", "name": "crates-io"}))

    def test_read_and_yank(self):
        self.accepted({key: value for key, value in self.request.items() if key not in ("name", "vers", "cksum")} | {"operation": "read"})
        self.rejected(dict(self.request, operation="yank"))

    def test_receipt_validation(self):
        self.accepted()
        self.receipt["crates"].pop()
        self.save()
        self.rejected(self.request)

    def test_environment_token_and_missing_token(self):
        self.accepted()
        del self.env["CARGO_REGISTRY_TOKEN"]
        self.rejected(self.request)

    def test_unset_cargo_home_refuses_fallback(self):
        self.accepted()
        del self.env["CARGO_REGISTRY_TOKEN"]
        self.env["HOME"] = str(self.root)
        cargo_home = self.root / ".cargo"
        cargo_home.mkdir()
        (cargo_home / "credentials.toml").write_text('[registry]\ntoken = "toy-canary"\n')
        self.rejected(self.request)

    def test_errors_redact_token(self):
        self.accepted()
        self.rejected(dict(self.request, operation="yank"))

    def file_token(self):
        del self.env["CARGO_REGISTRY_TOKEN"]
        self.env.update({"CARGO_HOME": str(self.root), "GREENNODE_RELEASE_CARGO_HOME": str(self.root),
                         "HOME": str(self.root / "home")})
        credentials = self.root / "credentials.toml"
        credentials.write_text('[registry]\ntoken = "toy-canary"\n')
        credentials.chmod(0o600)
        return credentials

    def test_designated_home_required(self):
        self.file_token()
        self.accepted()
        del self.env["GREENNODE_RELEASE_CARGO_HOME"]
        self.rejected(self.request)

    def test_designated_home_exact_match(self):
        self.file_token()
        self.accepted()
        self.env["GREENNODE_RELEASE_CARGO_HOME"] += "/."
        self.rejected(self.request)

    def test_ordinary_home_refused(self):
        self.file_token()
        self.accepted()
        home = self.root / "home"
        home.mkdir()
        self.env["HOME"] = str(home)
        alias = home / ".cargo"
        alias.symlink_to(self.root)
        self.rejected(self.request)

    def test_account_home_refused_when_home_environment_differs(self):
        self.file_token()
        self.accepted()
        home = self.root / "account"
        home.mkdir()
        (home / ".cargo").symlink_to(self.root)
        with patch("pwd.getpwuid", return_value=types.SimpleNamespace(pw_dir=str(home))):
            self.rejected(self.request)

    def test_directory_swap_keeps_approved_credentials(self):
        del self.env["CARGO_REGISTRY_TOKEN"]
        home = self.root / "approved"
        other = self.root / "other"
        home.mkdir()
        other.mkdir()
        for directory, value in ((home, "toy-canary"), (other, "different-toy")):
            file = directory / "credentials.toml"
            file.write_text('[registry]\ntoken = "' + value + '"\n')
            file.chmod(0o600)
        self.env.update({"CARGO_HOME": str(home), "GREENNODE_RELEASE_CARGO_HOME": str(home),
                         "HOME": str(self.root / "user")})
        self.accepted()
        saved = self.root / "saved"
        real_open = os.open

        def swap(path, flags, *args, **kwargs):
            if str(path) == str(home):
                fd = real_open(path, flags, *args, **kwargs)
                home.rename(saved)
                home.symlink_to(other)
                return fd
            if str(path) == str(home / "credentials.toml"):
                home.rename(saved)
                home.symlink_to(other)
            return real_open(path, flags, *args, **kwargs)

        with patch("os.open", side_effect=swap):
            self.accepted()
        self.assertTrue(home.is_symlink())

    def test_credentials_symlink_refused(self):
        credentials = self.file_token()
        self.accepted()
        target = self.root / "other"
        credentials.rename(target)
        credentials.symlink_to(target)
        self.rejected(self.request)

    def test_credentials_permissions_refused(self):
        credentials = self.file_token()
        self.accepted()
        credentials.chmod(0o640)
        self.rejected(self.request)

    def test_nonregular_credentials_refused(self):
        self.file_token()
        self.accepted()
        with patch("stat.S_ISREG", return_value=False):
            self.rejected(self.request)

    def test_oversized_credentials_refused(self):
        credentials = self.file_token()
        self.accepted()
        with credentials.open("a") as stream:
            stream.write("#" + "x" * 1048576)
        self.rejected(self.request)

    def test_credentials_owner_refused(self):
        self.file_token()
        self.accepted()
        with patch("os.getuid", side_effect=[os.getuid(), -1]):
            self.rejected(self.request)

    def test_protocol_version_is_integer(self):
        self.accepted()
        for value in (True, 1.0):
            with self.subTest(value=value):
                self.rejected(dict(self.request, v=value))

    def test_read_rejects_publish_fields(self):
        read = {key: value for key, value in self.request.items() if key not in ("name", "vers", "cksum")} | {"operation": "read"}
        self.accepted(read)
        self.rejected(dict(read, name="greennode-sdk"))

    def test_protocol_fields_and_types(self):
        self.accepted(dict(self.request, args=["option"], registry={"index-url": "sparse+https://index.crates.io/", "headers": ["header"]}))
        for index, request in enumerate((dict(self.request, unexpected=1), dict(self.request, args="bad"),
                        dict(self.request, registry={"index-url": "sparse+https://index.crates.io/", "extra": 1}),
                        dict(self.request, registry={"index-url": "sparse+https://index.crates.io/", "headers": [1]}))):
            with self.subTest(case=index):
                self.rejected(request)

    def test_registry_name_type(self):
        read = {"v": 1, "kind": "get", "operation": "read", "registry": {"index-url": "sparse+https://index.crates.io/", "name": "crates-io"}}
        self.accepted(read)
        self.rejected(dict(read, registry=dict(read["registry"], name=1)))

    def test_argument_element_type(self):
        self.accepted(dict(self.request, args=["ok"]))
        self.rejected(dict(self.request, args=[1]))

    def test_publish_field_types(self):
        provider = load()
        self.accepted()
        with patch.object(provider, "receipt", return_value=[{"name": 1, "version": "0.1.0", "sha256": "a" * 64}]):
            with self.assertRaises(ValueError):
                provider.response(dict(self.request, name=1), self.env)

    def test_cargo_home_token_and_malformed_credentials(self):
        del self.env["CARGO_REGISTRY_TOKEN"]
        self.env["CARGO_HOME"] = str(self.root)
        self.env["GREENNODE_RELEASE_CARGO_HOME"] = str(self.root)
        self.env["HOME"] = str(self.root / "home")
        credentials = self.root / "credentials.toml"
        credentials.write_text('[registry]\ntoken = "toy-canary"\n')
        credentials.chmod(0o600)
        self.accepted()
        credentials.write_text('[registry\n')
        self.rejected(self.request)


if __name__ == "__main__":
    unittest.main()
