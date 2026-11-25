# Usage: python3 -I scripts/check-crate-release.py receipt TAG STAGED_DIR RECEIPT --commit SHA --toolchain VERSION


import hashlib
import http.client
import json
import queue
import re
import sys
import tarfile
import threading
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
import zlib
import xml.etree.ElementTree as ET
from pathlib import Path, PurePosixPath


class GateError(ValueError):
    pass


class TransientError(Exception):
    pass


TAG = re.compile(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
SEMVER = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
                    r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
                    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?")
NAMES = ("greennode-sdk", "greennode")


def archive_data(name, version, data):
    prefix = f"{name}-{version}"
    required = {f"{prefix}/Cargo.toml", f"{prefix}/Cargo.lock"}
    found = {}
    seen = set()
    files = set()
    ancestors = set()
    try:
        inflater = zlib.decompressobj(16 + zlib.MAX_WBITS)
        raw = inflater.decompress(data, 50 * 1024 * 1024 + 1)
        if len(raw) > 50 * 1024 * 1024 or not inflater.eof or inflater.unused_data or inflater.unconsumed_tail:
            raise GateError(f"{name}: invalid gzip framing")
        if len(raw) % 512:
            raise GateError(f"{name}: invalid tar framing")
        offset = 0
        count = 0
        ended = False
        while offset + 512 <= len(raw):
            header = raw[offset:offset + 512]
            if header == bytes(512):
                if offset + 1024 > len(raw) or any(raw[offset:]):
                    raise GateError(f"{name}: invalid tar framing")
                ended = True
                break
            if header[156:157] not in (b"0", b"\0", b"5"):
                raise GateError(f"{name}: unsupported archive member")
            member = tarfile.TarInfo.frombuf(header, "utf-8", "strict")
            count += 1
            path = PurePosixPath(member.name)
            canonical = path.as_posix()
            if (count > 10000 or member.size > 16 * 1024 * 1024 or member.size < 0
                    or path.is_absolute() or ".." in path.parts or not path.parts
                    or path.parts[0] != prefix or canonical in seen or member.linkname
                    or (len(path.parts) == 1 and not member.isdir())):
                raise GateError(f"{name}: unsafe archive member")
            parents = {parent.as_posix() for parent in path.parents}
            if parents.intersection(files) or (member.isfile() and canonical in ancestors):
                raise GateError(f"{name}: conflicting archive member")
            seen.add(canonical)
            ancestors.update(parents)
            if member.isfile():
                files.add(canonical)
            start = offset + 512
            end = start + member.size
            offset = start + ((member.size + 511) // 512) * 512
            if end > len(raw) or offset > len(raw) or (member.isdir() and member.size):
                raise GateError(f"{name}: invalid tar framing")
            if canonical in required:
                if not member.isfile():
                    raise GateError(f"{name}: invalid archive metadata")
                found[canonical] = raw[start:end]
        if not ended:
            raise GateError(f"{name}: invalid tar framing")
        if set(found) != required:
            raise GateError(f"{name}: missing archive metadata")
        return (tomllib.loads(found[f"{prefix}/Cargo.toml"].decode("utf-8")),
                tomllib.loads(found[f"{prefix}/Cargo.lock"].decode("utf-8")))
    except (tarfile.TarError, UnicodeError, OSError, EOFError, tomllib.TOMLDecodeError, zlib.error) as error:
        raise GateError(f"{name}: invalid archive") from error


def sdk_declarations(data, name):
    targets = data.get("target", {})
    if not isinstance(targets, dict):
        raise GateError(f"{name}: invalid dependency tables")
    tables = [(True, data)] + [(False, table) for table in targets.values()]
    for root, table in tables:
        if not isinstance(table, dict):
            raise GateError(f"{name}: invalid dependency tables")
        for section in ("dependencies", "build-dependencies", "dev-dependencies"):
            dependencies = table.get(section, {})
            if not isinstance(dependencies, dict):
                raise GateError(f"{name}: invalid dependency table")
            for alias, dependency in dependencies.items():
                package = dependency.get("package", alias) if isinstance(dependency, dict) else alias
                if package == "greennode-sdk":
                    yield root and section == "dependencies", dependency


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
            declarations = list(sdk_declarations(manifest, name))
            allowed = {"version", "features", "default-features", "optional", "package"}
            if (not any(normal for normal, _ in declarations)
                    or any(not isinstance(dependency, dict) or dependency.get("version") != f"={version}"
                           or not set(dependency).issubset(allowed)
                           for _, dependency in declarations)):
                raise GateError("greennode: invalid SDK dependency")
            packages = lock.get("package", [])
            if not isinstance(packages, list):
                raise GateError("greennode: invalid SDK lockfile")
            matching = [p for p in packages if isinstance(p, dict) and p.get("name") == "greennode-sdk"]
            if (len(matching) != 1 or matching[0].get("version") != version
                    or matching[0].get("checksum") != sdk_hash
                    or matching[0].get("source") != "registry+https://github.com/rust-lang/crates.io-index"):
                raise GateError("greennode: invalid SDK lockfile checksum or source")
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


def fetch_url(url, timeout=30, allow_loopback=False, deadline=None, clock=None):
    parsed = urllib.parse.urlsplit(url)
    if parsed.scheme != "https" and not (allow_loopback and parsed.scheme == "http" and parsed.hostname == "127.0.0.1"):
        raise ValueError("HTTPS required")
    clock = clock or time.monotonic
    deadline = clock() + timeout if deadline is None else deadline
    if clock() >= deadline:
        raise TimeoutError()
    request = urllib.request.Request(url, headers={"User-Agent": "release-checker", "Cache-Control": "no-cache"})
    try:
        response = urllib.request.build_opener(NoRedirect()).open(request, timeout=timeout)
    except urllib.error.HTTPError as error:
        response = error
    except urllib.error.URLError as error:
        if isinstance(error.reason, (TimeoutError, ConnectionRefusedError)):
            raise error.reason from error
        raise GateError("greennode: transport failed") from error
    except ConnectionResetError as error:
        raise GateError("greennode: transport failed") from error
    try:
        with response:
            chunks = []
            size = 0
            while True:
                if clock() >= deadline:
                    raise TimeoutError()
                chunk = response.read1(min(65536, 1024 * 1024 + 1 - size))
                if clock() >= deadline:
                    raise TimeoutError()
                if not chunk:
                    break
                chunks.append(chunk)
                size += len(chunk)
                if size > 1024 * 1024:
                    raise ValueError("invalid response framing")
            data = b"".join(chunks)
            length = response.headers.get("Content-Length")
            if len(data) > 1024 * 1024 or (length is not None and (not length.isdecimal() or int(length) != len(data))):
                raise ValueError("invalid response framing")
            return response.status, data
    except (http.client.HTTPException, ConnectionResetError, ConnectionRefusedError) as error:
        raise ValueError("invalid response framing") from error


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


def api_not_found(data, name, version=None):
    payload = read_json(data)
    if not isinstance(payload, dict) or set(payload) != {"errors"}:
        return False
    errors = payload["errors"]
    if not isinstance(errors, list) or len(errors) != 1 or not isinstance(errors[0], dict):
        return False
    details = {f"crate `{name}` does not exist"}
    if version is not None:
        details.add(f"crate `{name}` does not have a version `{version}`")
    return set(errors[0]) == {"detail"} and errors[0]["detail"] in details


def index_not_found(data, name):
    key = f"{index_prefix(name)}/{name}"
    try:
        text = data.decode("utf-8")
        if "<!DOCTYPE" in text.upper() or "<!ENTITY" in text.upper():
            return False
        root = ET.fromstring(text)
        codes, keys = root.findall("Code"), root.findall("Key")
        return (root.tag == "Error" and len(codes) == len(keys) == 1
                and not list(codes[0]) and not list(keys[0])
                and codes[0].text == "NoSuchKey" and keys[0].text == key)
    except (UnicodeError, ET.ParseError):
        return False


def valid_semver(value):
    if not isinstance(value, str):
        return False
    match = SEMVER.fullmatch(value)
    if not match:
        return False
    if any(int(component) > 2**64 - 1 for component in match.group(1, 2, 3)):
        return False
    prerelease = match.group(4)
    return not prerelease or all(not (part.isdigit() and len(part) > 1 and part[0] == "0")
                                 for part in prerelease.split("."))


def index_record_valid(row, name):
    if (not isinstance(row, dict) or row.get("name") != name or not valid_semver(row.get("vers"))
            or not isinstance(row.get("deps"), list) or not isinstance(row.get("cksum"), str)
            or not re.fullmatch(r"[0-9a-f]{64}", row["cksum"]) or type(row.get("yanked")) is not bool):
        return False
    for dependency in row["deps"]:
        if (not isinstance(dependency, dict) or not isinstance(dependency.get("name"), str)
                or not dependency["name"] or not isinstance(dependency.get("req"), str) or not dependency["req"]):
            return False
        if any(type(dependency[key]) is not bool for key in ("optional", "default_features") if key in dependency):
            return False
        if "features" in dependency and (not isinstance(dependency["features"], list)
                or not all(isinstance(value, str) for value in dependency["features"])):
            return False
        if any(dependency[key] is not None and not isinstance(dependency[key], str)
               for key in ("target", "kind", "registry", "package") if key in dependency):
            return False
        if dependency.get("kind") not in (None, "normal", "build", "dev"):
            return False
    for key in ("features", "features2"):
        if key in row and (not isinstance(row[key], dict)
                or any(not isinstance(k, str) or not isinstance(v, list)
                       or not all(isinstance(item, str) for item in v) for k, v in row[key].items())):
            return False
    if "v" in row and (type(row["v"]) is not int or row["v"] not in (1, 2)):
        return False
    if "features2" in row and row.get("v", 1) < 2:
        return False
    for key in ("links", "rust_version", "pubtime"):
        if key in row and row[key] is not None and not isinstance(row[key], str):
            return False
    return True


def registry_state(record, fetch, first=False):
    name, version, digest = (record[key] for key in ("name", "version", "sha256"))
    api_status, api_data = fetch(f"https://crates.io/api/v1/crates/{name}/{version}")
    index_status, index_data = fetch(f"https://index.crates.io/{index_prefix(name)}/{name}")
    allowed_transient = (502, 503, 504)
    transient = api_status in allowed_transient or index_status in allowed_transient
    if (type(api_status) is not int or type(index_status) is not int
            or api_status not in (200, 404, *allowed_transient)
            or index_status not in (200, 404, *allowed_transient)):
        return "stop"
    if api_status == 404 and not api_not_found(api_data, name, version):
        return "stop"
    if index_status == 404 and not index_not_found(index_data, name):
        return "stop"
    if first and index_status == 200:
        return "stop"
    matches = []
    if index_status == 200:
        if not index_data.endswith(b"\n"):
            return "stop"
        for line in index_data.splitlines():
            row = read_json(line)
            if not index_record_valid(row, name):
                return "stop"
            if row["vers"] == version:
                matches.append(row)
    if len(matches) > 1:
        return "stop"
    if matches and (matches[0].get("cksum") != digest or matches[0].get("yanked") is not False):
        return "stop"
    if api_status == 404 and not matches and not transient:
        return "absent"
    if api_status == 200:
        api = read_json(api_data)
        if not isinstance(api, dict) or not isinstance(api.get("version"), dict):
            return "stop"
        api = api["version"]
        if (api.get("crate") != name or not valid_semver(api.get("num")) or api.get("num") != version
                or api.get("checksum") != digest or api.get("yanked") is not False):
            return "stop"
    if transient:
        return "transient"
    if api_status != 200 or not matches:
        return "stop"
    return "present-and-approved"


def deadline_fetch(fetch, timeout):
    result = queue.Queue(maxsize=1)

    def run():
        try:
            result.put((True, fetch()))
        except Exception as error:
            result.put((False, error))

    threading.Thread(target=run, daemon=True).start()
    try:
        success, value = result.get(timeout=timeout)
    except queue.Empty as error:
        raise TimeoutError() from error
    if not success:
        raise value
    return value


def state(path, first=False, wait=None, fetch=None, sleep=None, clock=None):
    network = fetch or fetch_url
    sleep = sleep or time.sleep
    clock = clock or time.monotonic
    deadline = clock() + 60

    def timed_fetch(url):
        remaining = deadline - clock()
        if remaining <= 0:
            raise TimeoutError()
        try:
            result = deadline_fetch(
                lambda: (fetch_url(url, timeout=min(30, remaining), deadline=deadline, clock=clock)
                         if fetch is None else network(url)), remaining)
        except (TimeoutError, ConnectionRefusedError, ConnectionResetError) as error:
            raise TransientError() from error
        if clock() >= deadline:
            raise TimeoutError()
        return result

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
                result = registry_state(record, timed_fetch, first=first)
                if first and result == "absent":
                    status, data = timed_fetch(f"https://crates.io/api/v1/crates/{name}")
                    if status != 404 or not api_not_found(data, name):
                        result = "stop"
                elif first:
                    result = "stop"
            except (TransientError, TimeoutError):
                result = "transient"
            except Exception:
                result = "stop"
            if name != wait or result not in ("transient", "absent"):
                break
            remaining = deadline - clock()
            if attempt + 1 < attempts and remaining > 0:
                sleep(min(2, remaining))
            else:
                break
        if name == wait and result != "present-and-approved":
            result = "unknown"
        elif result == "transient":
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
        if args and args[0] == "shipped" and len(args) == 4:
            candidate, committed, previous = args[1:]
            if previous == "--first":
                shipped(Path(candidate), Path(committed), first=True)
            else:
                shipped(Path(candidate), Path(committed), Path(previous))
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


def shipped(candidate, committed, previous=None, first=False):
    if (previous is None) == (not first):
        raise GateError("greennode: previous inventory or first required")
    builtins = {"greennode", "greennode --help", "greennode --version", "greennode -h",
                "greennode -v", "greennode help", "greennode version"}

    def inventory(path):
        value = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
            raise GateError("greennode: invalid shipped inventory")
        def valid_path(item):
            if item in builtins:
                return True
            parts = item.split(" ")
            if (len(parts) not in (2, 3) or parts[0] != "greennode"
                    or parts[1] in {"gen-docs", "help", "version"}
                    or any(not re.fullmatch(r"[a-z0-9][a-z0-9-]*", part) for part in parts[1:])):
                return False
            return len(parts) == 2 or " ".join(parts[:2]) in value

        if (value != sorted(set(value)) or not builtins.issubset(value)
                or any(not valid_path(item) for item in value)):
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
    dependencies = workspace["workspace"].get("dependencies", {})
    if not isinstance(dependencies, dict):
        raise GateError("greennode: invalid workspace dependencies")
    for alias, dependency in dependencies.items():
        package = dependency.get("package", alias) if isinstance(dependency, dict) else alias
        if package == "greennode-sdk":
            raise GateError("greennode: SDK workspace dependency refused")
    if tag is not None and tag != f"v{version}":
        raise GateError("greennode: workspace version must match tag")
    for name in ("greennode-sdk", "greennode"):
        path = root / "crates" / name / "Cargo.toml"
        data = tomllib.loads(path.read_text(encoding="utf-8"))
        package = data["package"]
        if package.get("name") != name or package.get("version") != {"workspace": True}:
            raise GateError(f"{name}: workspace version required")
        normal_sdk = False
        for normal, dependency in sdk_declarations(data, name):
            normal_sdk |= normal
            if (not isinstance(dependency, dict) or dependency.get("workspace") is True
                    or dependency.get("path") != "../greennode-sdk"
                    or dependency.get("version") != f"={version}"):
                raise GateError(f"{name}: exact SDK path and pin required")
        if name == "greennode" and release and not normal_sdk:
            raise GateError("greennode: SDK dependency required for release")
    return None


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
