# Usage: python3 -I scripts/test/check-crate-publish-credential-test.py
import types
import io
import json
import tempfile
import unittest
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
        self.accepted(dict(self.request, operation="read"))
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

    def test_cargo_home_token_and_malformed_credentials(self):
        del self.env["CARGO_REGISTRY_TOKEN"]
        self.env["CARGO_HOME"] = str(self.root)
        credentials = self.root / "credentials.toml"
        credentials.write_text('[registry]\ntoken = "toy-canary"\n')
        self.accepted()
        credentials.write_text('[registry\n')
        self.rejected(self.request)


if __name__ == "__main__":
    unittest.main()
