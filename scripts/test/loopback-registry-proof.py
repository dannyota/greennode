import hashlib
import http.server
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import threading


ROOT = Path(__file__).resolve().parents[2]
PUBLISH_CONFIG = ROOT / "scripts/publish-cargo.toml"
TOKEN = "toy-loopback-token"
NAMES = ("greennode-sdk", "greennode")
VERSION = "0.1.0"


def upload_args(name, config):
    return ("publish", "--no-verify", "--locked", "--registry", "crates-io", "--package", name,
            "--config", str(config), "--config", "net.retry=0")


def digest(data):
    return hashlib.sha256(data).hexdigest()


class Registry(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, status, body=b""):
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/index/config.json":
            body = json.dumps({"dl": self.server.base + "/download/{crate}/{version}",
                               "api": self.server.base}).encode()
            self.reply(200, body)
        elif self.path.startswith("/index/gr/ee/") and self.path.rsplit("/", 1)[-1] in self.server.archives:
            name = self.path.rsplit("/", 1)[-1]
            entry = {"name": name, "vers": VERSION, "deps": [],
                     "cksum": digest(self.server.archives[name]), "features": {}, "yanked": False}
            self.reply(200, (json.dumps(entry) + "\n").encode())
        elif self.path == f"/download/{NAMES[0]}/{VERSION}" and self.server.indexed:
            self.reply(200, self.server.archives[NAMES[0]])
        else:
            self.reply(404)

    def do_PUT(self):
        self.server.attempts += 1
        if self.path != "/api/v1/crates/new" or self.headers.get("Authorization") != TOKEN:
            self.reply(403)
            return
        data = self.rfile.read(int(self.headers["Content-Length"]))
        size = struct.unpack_from("<I", data)[0]
        metadata = json.loads(data[4:4 + size])
        offset = 4 + size
        length = struct.unpack_from("<I", data, offset)[0]
        archive = data[offset + 4:]
        if len(archive) != length:
            self.reply(400)
            return
        self.server.archives[metadata["name"]] = archive
        self.server.uploads.append(metadata["name"])
        self.reply(200, b'{"ok":true}')


def run_case(root, cargo, rustup_home, source):
    root.mkdir()
    home = root / "home"
    cargo_home = root / "cargo-home"
    workspace = root / "workspace"
    for path in (home, cargo_home, workspace):
        path.mkdir()
    for directory in (workspace.resolve(), *workspace.resolve().parents):
        for name in ("config", "config.toml"):
            if os.path.lexists(directory / ".cargo" / name):
                raise RuntimeError(f"Cargo configuration in workspace ancestor: {directory / '.cargo' / name}")
    for name in NAMES:
        crate = workspace / name
        (crate / "src").mkdir(parents=True)
        dependency = ('\n[dependencies]\ngreennode-sdk = { path = "../greennode-sdk", version = "=0.1.0" }\n'
                      if name == NAMES[1] else "")
        (crate / "Cargo.toml").write_text(f'[package]\nname = "{name}"\nversion = "{VERSION}"\nedition = "2021"\nlicense = "MIT"\n{dependency}')
        (crate / "src/lib.rs").write_text("pub fn toy() {}\n")
    manifest = workspace / "Cargo.toml"
    manifest.write_text('[workspace]\nmembers = ["greennode-sdk", "greennode"]\nresolver = "2"\n')
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Registry)
    server.base = f"http://127.0.0.1:{server.server_port}"
    server.indexed = False
    server.archives = {}
    server.uploads = []
    server.attempts = 0
    thread = threading.Thread(target=server.serve_forever)
    thread.start()
    try:
        index = "sparse+" + server.base + "/index/"
        receipt = root / "receipt.json"
        env = {"PATH": os.environ["PATH"], "HOME": str(home), "CARGO_HOME": str(cargo_home),
               "RUSTUP_HOME": rustup_home, "RUSTUP_AUTO_INSTALL": "0", "CARGO_BUILD_JOBS": "4",
               "GREENNODE_RELEASE_CARGO_HOME": str(cargo_home), "GREENNODE_RELEASE_RECEIPT": str(receipt),
               "CARGO_HTTP_PROXY": server.base, "NO_PROXY": "127.0.0.1",
               "__CARGO_TEST_CRATES_IO_URL_DO_NOT_USE_THIS": index}
        if source == "environment":
            env["CARGO_REGISTRY_TOKEN"] = TOKEN
        else:
            credentials = cargo_home / "credentials.toml"
            credentials.write_text(f'[registry]\ntoken = "{TOKEN}"\n')
            credentials.chmod(0o600)

        def cargo_run(*args, success=True):
            result = subprocess.run([cargo, "+1.100.0", *args],
                                    cwd=workspace, env=env, text=True, capture_output=True, timeout=120)
            assert TOKEN not in result.stdout and TOKEN not in result.stderr
            if (result.returncode == 0) != success:
                raise RuntimeError(f"cargo {args[0]} unexpected exit {result.returncode}:\n{result.stderr}")
            return result

        cargo_run("generate-lockfile")
        build = root / "build"
        env["CARGO_TARGET_DIR"] = str(root / "target")
        env["CARGO_BUILD_BUILD_DIR"] = str(build)
        cargo_run("publish", "--workspace", "--dry-run", "--no-verify", "--locked", "--registry", "crates-io")
        staged = {name: (build / "package/tmp-crate" / f"{name}-{VERSION}.crate").read_bytes() for name in NAMES}
        hashes = {name: digest(data) for name, data in staged.items()}

        def write_receipt(bad=False):
            crates = [{"name": name, "version": VERSION, "filename": f"{name}-{VERSION}.crate",
                       "sha256": "0" * 64 if bad else hashes[name]} for name in NAMES]
            receipt.write_text(json.dumps({"tag": "v0.1.0", "commit": "0" * 40,
                                           "toolchain": "1.100.0", "crates": crates}))

        for name in NAMES:
            upload = upload_args(name, PUBLISH_CONFIG)
            previous_attempts = server.attempts
            write_receipt(True)
            refused = cargo_run(*upload, success=False)
            if "credential request refused" not in refused.stderr:
                raise RuntimeError(f"credential provider did not refuse the receipt for {name}:\n{refused.stderr}")
            assert server.attempts == previous_attempts
            write_receipt()
            cargo_run(*upload)
            assert server.attempts == previous_attempts + 1
            assert digest(server.archives[name]) == hashes[name]
            server.indexed = True
        assert server.uploads == list(NAMES)
        print(f"{source}: mismatched receipts refused; both uploads matched staged checksums")
    finally:
        server.shutdown()
        thread.join()
        server.server_close()


def main():
    cargo = shutil.which("cargo")
    if not cargo:
        raise RuntimeError("cargo not found")
    rustup_home = os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup"))
    with tempfile.TemporaryDirectory(prefix="loopback-registry-proof-") as temp:
        for source in ("environment", "credentials-file"):
            run_case(Path(temp) / source, cargo, rustup_home, source)


if __name__ == "__main__":
    main()
