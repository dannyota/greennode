# Usage: python3 -I scripts/check-crate-release.py receipt TAG STAGED_DIR RECEIPT --commit SHA --toolchain VERSION


import hashlib
import io
import json
import re
import sys
import tarfile
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path, PurePosixPath


class GateError(ValueError):
    pass


TAG = re.compile(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
NAMES = ("greennode-sdk", "greennode")


def archive_data(name, version, data):
    prefix = f"{name}-{version}"
    required = {f"{prefix}/Cargo.toml", f"{prefix}/Cargo.lock"}
    found = {}
    try:
        with tarfile.open(fileobj=io.BytesIO(data), mode="r|gz") as archive:
            for count, member in enumerate(archive, 1):
                path = PurePosixPath(member.name)
                if (count > 10000 or member.size > 16 * 1024 * 1024 or member.size < 0
                        or path.is_absolute() or ".." in path.parts or not path.parts
                        or path.parts[0] != prefix or not (member.isfile() or member.isdir())):
                    raise GateError(f"{name}: unsafe archive member")
                if member.name in required:
                    if member.name in found or not member.isfile():
                        raise GateError(f"{name}: invalid archive metadata")
                    found[member.name] = archive.extractfile(member).read()
        if set(found) != required:
            raise GateError(f"{name}: missing archive metadata")
        return (tomllib.loads(found[f"{prefix}/Cargo.toml"].decode("utf-8")),
                tomllib.loads(found[f"{prefix}/Cargo.lock"].decode("utf-8")))
    except (tarfile.TarError, UnicodeError, OSError, EOFError, tomllib.TOMLDecodeError) as error:
        raise GateError(f"{name}: invalid archive") from error


def receipt(tag, staged, target, commit, toolchain):
    if not isinstance(tag, str) or not TAG.fullmatch(tag):
        raise GateError("greennode: invalid tag")
    if not isinstance(commit, str) or not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise GateError("greennode: invalid commit")
    if not isinstance(toolchain, str) or not VERSION.fullmatch(toolchain):
        raise GateError("greennode: invalid toolchain")
    staged, target = Path(staged), Path(target)
    if target.exists() or target.is_symlink():
        raise GateError("greennode: receipt already exists")
    version = tag[1:]
    filenames = [f"{name}-{version}.crate" for name in NAMES]
    if staged.is_symlink() or not staged.is_dir() or {p.name for p in staged.iterdir()} != set(filenames):
        raise GateError("greennode: invalid staged files")
    records = []
    sdk_hash = None
    for name, filename in zip(NAMES, filenames):
        path = staged / filename
        if path.is_symlink() or not path.is_file() or path.stat().st_size > 128 * 1024 * 1024:
            raise GateError(f"{name}: invalid staged archive")
        data = path.read_bytes()
        digest = hashlib.sha256(data).hexdigest()
        manifest, lock = archive_data(name, version, data)
        package = manifest.get("package", {})
        if not isinstance(package, dict) or package.get("name") != name or package.get("version") != version:
            raise GateError(f"{name}: package name or version mismatch")
        if name == "greennode-sdk":
            sdk_hash = digest
        else:
            dependencies = manifest.get("dependencies", {})
            dependency = dependencies.get("greennode-sdk") if isinstance(dependencies, dict) else None
            allowed = {"version", "features", "default-features", "optional"}
            if (not isinstance(dependency, dict) or dependency.get("version") != f"={version}"
                    or not set(dependency).issubset(allowed)):
                raise GateError("greennode: invalid SDK dependency")
            packages = lock.get("package", [])
            if not isinstance(packages, list):
                raise GateError("greennode: invalid SDK lockfile")
            matching = [p for p in packages if isinstance(p, dict) and p.get("name") == "greennode-sdk"]
            if len(matching) != 1 or matching[0].get("version") != version or matching[0].get("checksum") != sdk_hash:
                raise GateError("greennode: invalid SDK lockfile checksum")
        records.append({"name": name, "version": version, "filename": filename, "sha256": digest})
    result = {"tag": tag, "commit": commit, "toolchain": toolchain, "crates": records}
    try:
        with target.open("x", encoding="utf-8") as output:
            output.write(json.dumps(result, indent=2) + "\n")
    except FileExistsError as error:
        raise GateError("greennode: receipt already exists") from error


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def fetch_url(url):
    if urllib.parse.urlsplit(url).scheme != "https":
        raise ValueError("HTTPS required")
    request = urllib.request.Request(url, headers={"User-Agent": "release-checker", "Cache-Control": "no-cache"})
    try:
        with urllib.request.build_opener(NoRedirect()).open(request, timeout=30) as response:
            data = response.read(1024 * 1024 + 1)
            if len(data) > 1024 * 1024:
                raise ValueError("response too large")
            return response.status, data
    except urllib.error.HTTPError as error:
        return error.code, b""


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate key")
        result[key] = value
    return result


def read_json(data):
    return json.loads(data, object_pairs_hook=unique_object)


def validate_receipt(path):
    data = read_json(Path(path).read_bytes())
    if not isinstance(data, dict) or set(data) != {"tag", "commit", "toolchain", "crates"}:
        raise ValueError("invalid receipt")
    tag, commit, toolchain, crates = (data[key] for key in ("tag", "commit", "toolchain", "crates"))
    if (not isinstance(tag, str) or not TAG.fullmatch(tag) or not isinstance(commit, str)
            or not re.fullmatch(r"[0-9a-f]{40}", commit) or not isinstance(toolchain, str)
            or not VERSION.fullmatch(toolchain) or not isinstance(crates, list) or len(crates) != 2):
        raise ValueError("invalid receipt")
    for name, record in zip(NAMES, crates):
        if (not isinstance(record, dict) or set(record) != {"name", "version", "filename", "sha256"}
                or record["name"] != name or record["version"] != tag[1:]
                or record["filename"] != f"{name}-{tag[1:]}.crate"
                or not isinstance(record["sha256"], str)
                or not re.fullmatch(r"[0-9a-f]{64}", record["sha256"])):
            raise ValueError("invalid receipt")
    return crates


def index_prefix(name):
    name = name.lower()
    if len(name) == 1:
        return "1"
    if len(name) == 2:
        return "2"
    if len(name) == 3:
        return f"3/{name[0]}"
    return f"{name[:2]}/{name[2:4]}"


def registry_state(record, fetch):
    name, version, digest = (record[key] for key in ("name", "version", "sha256"))
    api_status, api_data = fetch(f"https://crates.io/api/v1/crates/{name}/{version}")
    index_status, index_data = fetch(f"https://index.crates.io/{index_prefix(name)}/{name}")
    if api_status not in (200, 404) or index_status not in (200, 404):
        return "stop"
    matches = []
    if index_status == 200:
        if not index_data.splitlines():
            return "stop"
        for line in index_data.splitlines():
            row = read_json(line)
            if (not isinstance(row, dict) or row.get("name") != name or not isinstance(row.get("vers"), str)
                    or not isinstance(row.get("cksum"), str)
                    or not re.fullmatch(r"[0-9a-f]{64}", row["cksum"])
                    or type(row.get("yanked")) is not bool):
                return "stop"
            if row["vers"] == version:
                matches.append(row)
    if api_status == 404 and not matches:
        return "absent"
    if len(matches) > 1:
        return "stop"
    if api_status != 200 or not matches:
        return "pending"
    api = read_json(api_data)
    if not isinstance(api, dict) or not isinstance(api.get("version"), dict):
        return "stop"
    api = api["version"]
    index = matches[0]
    if (api.get("crate") != name or api.get("num") != version or api.get("checksum") != digest
            or api.get("yanked") is not False or index.get("cksum") != digest
            or index.get("yanked") is not False):
        return "stop"
    return "present-and-approved"


def state(path, first=False, wait=None, fetch=None, sleep=None):
    fetch = fetch or fetch_url
    sleep = sleep or time.sleep
    try:
        records = validate_receipt(path)
        if wait is not None and wait not in NAMES:
            raise ValueError("invalid wait crate")
    except (OSError, ValueError, TypeError):
        print("greennode: invalid receipt or state arguments", file=sys.stderr)
        return 1
    results = {}
    for record in records:
        name = record["name"]
        attempts = 5 if name == wait else 1
        for attempt in range(attempts):
            try:
                result = registry_state(record, fetch)
                if first and result == "absent":
                    status, _ = fetch(f"https://crates.io/api/v1/crates/{name}")
                    if status != 404:
                        result = "stop"
                elif first:
                    result = "stop"
            except OSError:
                result = "pending" if name == wait else "stop"
            except (ValueError, TypeError, KeyError):
                result = "stop"
            if name != wait or result in ("present-and-approved", "stop"):
                break
            if attempt + 1 < attempts:
                sleep(2)
        if name == wait and result in ("absent", "pending"):
            result = "unknown"
        elif result == "pending":
            result = "stop"
        results[name] = result
    for record in records:
        name = record["name"]
        suffix = f" version={record['version']} sha256={record['sha256']}" if results[name] == "unknown" else ""
        print(f"{name}: {results[name]}{suffix}")
    return int(any(value in ("stop", "unknown") for value in results.values()))


def main(args, root=None, fetch=None, sleep=None):
    if len(args) == 8 and args[0] == "receipt" and args[4] == "--commit" and args[6] == "--toolchain":
        try:
            receipt(args[1], args[2], args[3], args[5], args[7])
            return 0
        except GateError as error:
            print(str(error), file=sys.stderr)
            return 1
        except (OSError, ValueError):
            print("greennode: receipt validation failed", file=sys.stderr)
            return 1
    try:
        if args and args[0] == "manifest":
            rest = args[1:]
            tag = None
            release = False
            while rest:
                if rest[0] == "--release" and not release:
                    release, rest = True, rest[1:]
                elif rest[0] == "--tag" and tag is None and len(rest) >= 2:
                    tag, rest = rest[1], rest[2:]
                else:
                    raise GateError("greennode: invalid manifest arguments")
            manifest(Path(root) if root is not None else Path.cwd(), tag, release)
            return 0
        if args and args[0] == "shipped" and len(args) in (3, 4):
            shipped(*(Path(arg) for arg in args[1:]))
            return 0
        if args and args[0] == "state" and len(args) >= 2:
            rest = args[2:]
            first = False
            wait = None
            while rest:
                if rest[0] == "--first" and not first:
                    first, rest = True, rest[1:]
                elif rest[0] == "--wait" and wait is None and len(rest) >= 2:
                    wait, rest = rest[1], rest[2:]
                else:
                    raise GateError("greennode: invalid state arguments")
            if first and wait:
                raise GateError("greennode: invalid state arguments")
            return state(args[1], first=first, wait=wait, fetch=fetch, sleep=sleep)
    except GateError as error:
        print(str(error), file=sys.stderr)
        return 1
    except (OSError, ValueError, KeyError, TypeError):
        print("greennode: invalid command input", file=sys.stderr)
        return 1
    print("greennode: release checker command unavailable", file=sys.stderr)
    return 1


def shipped(candidate, committed, previous=None):
    def inventory(path):
        value = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
            raise GateError("greennode: invalid shipped inventory")
        if value != sorted(set(value)):
            raise GateError("greennode: invalid shipped inventory")
        return value

    current = inventory(committed)
    if inventory(candidate) != current or candidate.read_bytes() != committed.read_bytes():
        raise GateError("greennode: candidate inventory differs")
    if previous is not None and not set(inventory(previous)).issubset(current):
        raise GateError("greennode: previous inventory is not a subset")


def manifest(root, tag=None, release=False):
    workspace = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    version = workspace["workspace"]["package"]["version"]
    if tag is not None and tag != f"v{version}":
        raise GateError("greennode: workspace version must match tag")
    for name in ("greennode-sdk", "greennode"):
        path = root / "crates" / name / "Cargo.toml"
        data = tomllib.loads(path.read_text(encoding="utf-8"))
        package = data["package"]
        if package.get("name") != name or package.get("version") != {"workspace": True}:
            raise GateError(f"{name}: workspace version required")
        if name == "greennode":
            dependency = data.get("dependencies", {}).get("greennode-sdk")
            if dependency is None and release:
                raise GateError("greennode: SDK dependency required for release")
            if dependency is not None and (not isinstance(dependency, dict)
                    or dependency.get("path") != "../greennode-sdk"
                    or dependency.get("version") != f"={version}"):
                raise GateError("greennode: exact SDK path and pin required")
    return None


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
