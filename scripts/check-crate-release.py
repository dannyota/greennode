# Usage: python3 -I scripts/check-crate-release.py receipt TAG STAGED_DIR RECEIPT --commit SHA --toolchain VERSION


import datetime
import hashlib
import http.client
import json
import os
import queue
import re
import ssl
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


def fetch_url(url, timeout=30, allow_loopback=False, deadline=None, clock=None, status_seen=None, ssl_context=None):
    parsed = urllib.parse.urlsplit(url)
    if parsed.scheme != "https" and not (allow_loopback and parsed.scheme == "http" and parsed.hostname == "127.0.0.1"):
        raise ValueError("HTTPS required")
    clock = clock or time.monotonic
    deadline = clock() + timeout if deadline is None else deadline
    if clock() >= deadline:
        raise TimeoutError()
    status_seen = status_seen or threading.Event()

    class Response(http.client.HTTPResponse):
        def _read_status(self):
            try:
                result = super()._read_status()
            except TimeoutError as error:
                if status_seen.is_set():
                    raise GateError("greennode: response timed out") from error
                raise TransientError() from error
            status_seen.set()
            return result

    class Connect:
        response_class = Response

        def connect(self):
            try:
                return super().connect()
            except TimeoutError as error:
                raise TransientError() from error

    class HTTPConnection(Connect, http.client.HTTPConnection):
        pass

    class HTTPSConnection(Connect, http.client.HTTPSConnection):
        pass

    class HTTPHandler(urllib.request.HTTPHandler):
        def http_open(self, req):
            return self.do_open(HTTPConnection, req)

    context = ssl_context if ssl_context is not None else (ssl.create_default_context() if parsed.scheme == "https" else None)

    class HTTPSHandler(urllib.request.HTTPSHandler):
        def https_open(self, req):
            return self.do_open(HTTPSConnection, req, context=context)

    request = urllib.request.Request(url, headers={"User-Agent": "release-checker", "Cache-Control": "no-cache"})
    try:
        response = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect(), HTTPHandler(), HTTPSHandler(context=context)).open(request, timeout=timeout)
    except urllib.error.HTTPError as error:
        response = error
    except urllib.error.URLError as error:
        if isinstance(error.reason, ConnectionRefusedError):
            raise error.reason from error
        raise GateError("greennode: transport failed") from error
    except (ConnectionResetError, TimeoutError) as error:
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
    except (http.client.HTTPException, ConnectionResetError, ConnectionRefusedError, TimeoutError) as error:
        raise GateError("greennode: invalid response framing") from error


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate key")
        result[key] = value
    return result


def reject_json_constant(value):
    raise ValueError("invalid JSON constant")


def read_json(data):
    try:
        value = json.loads(data, object_pairs_hook=unique_object, parse_constant=reject_json_constant)
        stack = [iter((value,))]
        while stack:
            try:
                item = next(stack[-1])
            except StopIteration:
                stack.pop()
                continue
            if isinstance(item, (dict, list)):
                if len(stack) > 64:
                    raise ValueError("invalid JSON depth")
                stack.append(iter(item.values() if isinstance(item, dict) else item))
        return value
    except RecursionError as error:
        raise ValueError("invalid JSON depth") from error


def validate_receipt_data(data):
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


def validate_receipt(path):
    return validate_receipt_data(read_json(Path(path).read_bytes()))


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
    return (isinstance(row, dict) and row.get("name") == name and valid_semver(row.get("vers"))
            and isinstance(row.get("cksum"), str) and re.fullmatch(r"[0-9a-f]{64}", row["cksum"]) is not None
            and type(row.get("yanked")) is bool)


def registry_state(record, fetch, first=False):
    name, version, digest = (record[key] for key in ("name", "version", "sha256"))
    api_status, api_data = fetch(f"https://crates.io/api/v1/crates/{name}/{version}")
    allowed_transient = (502, 503, 504)
    if type(api_status) is not int or api_status not in (200, 404, *allowed_transient):
        return "stop"
    if api_status == 404 and not api_not_found(api_data, name, version):
        return "stop"
    if api_status == 200:
        api = read_json(api_data)
        if not isinstance(api, dict) or not isinstance(api.get("version"), dict):
            return "stop"
        api = api["version"]
        if (api.get("crate") != name or not valid_semver(api.get("num")) or api.get("num") != version
                or api.get("checksum") != digest or api.get("yanked") is not False):
            return "stop"
    index_status, index_data = fetch(f"https://index.crates.io/{index_prefix(name)}/{name}")
    if type(index_status) is not int or index_status not in (200, 404, *allowed_transient):
        return "stop"
    transient = api_status in allowed_transient or index_status in allowed_transient
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
    if transient:
        return "transient"
    if api_status != 200 or not matches:
        return "stop"
    return "present-and-approved"


def deadline_fetch(fetch, timeout, status_seen=None):
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
        if status_seen is not None and status_seen.is_set():
            raise GateError("greennode: response timed out") from error
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
        status_seen = threading.Event()
        try:
            result = deadline_fetch(
                lambda: (fetch_url(url, timeout=min(30, remaining), deadline=deadline, clock=clock,
                                   status_seen=status_seen) if fetch is None else network(url)),
                remaining, status_seen=status_seen)
        except (TimeoutError, ConnectionRefusedError, ConnectionResetError) as error:
            raise TransientError() from error
        if clock() >= deadline:
            if status_seen.is_set():
                raise GateError("greennode: response timed out")
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


def artifact_entries_match(entries, expected):
    remaining = set(expected)
    for path in entries:
        if path.name not in remaining:
            return False
        remaining.remove(path.name)
    return not remaining


def tagged_tree(tree):
    if not isinstance(tree, bytes) or not tree or len(tree) > 8 * 1024 * 1024 or not tree.endswith(b"\0"):
        raise GateError("greennode: invalid tagged tree")
    paths = set()
    for entry in tree[:-1].split(b"\0"):
        try:
            header, path = entry.split(b"\t", 1)
            mode, kind, oid = header.split(b" ")
        except ValueError as error:
            raise GateError("greennode: invalid tagged tree") from error
        parts = path.split(b"/")
        if (mode not in (b"100644", b"100755") or kind != b"blob"
                or not re.fullmatch(rb"[0-9a-f]{40}", oid) or not path or path in paths
                or any(part in (b"", b".", b"..") for part in parts)):
            raise GateError("greennode: unsafe tagged tree")
        paths.add(path)
        if (parts[-1].lower() in (b"config", b"config.toml")
                and b".cargo" in [part.lower() for part in parts[:-1]]):
            raise GateError("greennode: Cargo config in tagged tree")


def evidence(tag, commit, release, runs, owners, artifacts, tree, publish=False):
    if not isinstance(tag, str) or not TAG.fullmatch(tag):
        raise GateError("greennode: invalid release tag")
    if not isinstance(commit, str) or not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise GateError("greennode: invalid release commit")
    if (not isinstance(release, dict) or release.get("tagName") != tag or release.get("isDraft") is not False
            or release.get("isImmutable") is not True):
        raise GateError("greennode: invalid release publication")
    published_at = release.get("publishedAt")
    try:
        if not isinstance(published_at, str) or not re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z", published_at):
            raise ValueError()
        datetime.datetime.strptime(published_at, "%Y-%m-%dT%H:%M:%SZ")
    except ValueError as error:
        raise GateError("greennode: invalid release publication") from error
    if not isinstance(runs, list) or len(runs) != 1 or not isinstance(runs[0], dict):
        raise GateError("greennode: expected one release run")
    run = runs[0]
    if (type(run.get("databaseId")) is not int or run["databaseId"] <= 0 or run.get("headBranch") != tag
            or run.get("headSha") != commit or run.get("path") != ".github/workflows/release.yml"
            or run.get("conclusion") != "success" or run.get("event") != "push"):
        raise GateError("greennode: release run identity mismatch")
    if not isinstance(owners, dict) or set(owners) != set(NAMES):
        raise GateError("greennode: invalid owner evidence")
    unregistered = []
    for name in NAMES:
        owner = owners[name]
        if owner is None:
            if publish:
                raise GateError(f"{name}: registered owner required")
            unregistered.append(name)
            continue
        users = owner.get("users") if isinstance(owner, dict) else None
        if (not isinstance(users, list) or not all(isinstance(user, dict) for user in users)
                or not any(user.get("kind") == "user" and user.get("login") == "dannyota" for user in users)):
            raise GateError(f"{name}: required owner missing")
    tagged_tree(tree)
    assets = release.get("assets")
    if not isinstance(assets, list) or not all(isinstance(asset, dict) for asset in assets):
        raise GateError("greennode: invalid release assets")
    by_name = {}
    for asset in assets:
        name = asset.get("name")
        if not isinstance(name, str) or name in by_name:
            raise GateError("greennode: invalid release asset names")
        by_name[name] = asset
    layouts = {"darwin-arm64": "tar.gz", "linux-amd64": "tar.gz",
               "linux-arm64": "tar.gz", "windows-amd64": "zip"}
    expected = {platform: {f"greennode-{platform}.{extension}", f"greennode-{platform}.sha256"}
                for platform, extension in layouts.items()}
    expected["crate-checksums"] = {"crate-checksums.json"}
    if set(by_name) != set().union(*expected.values()):
        raise GateError("greennode: unexpected release assets")
    artifacts = Path(artifacts)
    if artifacts.is_symlink() or not artifacts.is_dir():
        raise GateError("greennode: invalid artifact directories")
    with os.scandir(artifacts) as entries:
        if not artifact_entries_match(entries, set(expected)):
            raise GateError("greennode: invalid artifact directories")
    for directory, names in expected.items():
        folder = artifacts / directory
        if folder.is_symlink() or not folder.is_dir():
            raise GateError("greennode: invalid release artifact")
        with os.scandir(folder) as entries:
            if not artifact_entries_match(entries, names):
                raise GateError("greennode: invalid release artifact")
        for name in names:
            path = folder / name
            asset = by_name.get(name, {})
            if path.is_symlink() or not path.is_file() or asset.get("state") != "uploaded":
                raise GateError("greennode: missing release artifact")
            limit = 1024 * 1024 if name == "crate-checksums.json" or name.endswith(".sha256") else 128 * 1024 * 1024
            try:
                if path.stat().st_size > limit:
                    raise GateError("greennode: release artifact too large")
                with path.open("rb") as stream:
                    if name == "crate-checksums.json":
                        receipt_bytes = stream.read(limit + 1)
                        if len(receipt_bytes) > limit:
                            raise GateError("greennode: invalid release receipt")
                        digest = "sha256:" + hashlib.sha256(receipt_bytes).hexdigest()
                    else:
                        hashed = hashlib.sha256()
                        total = 0
                        while True:
                            data = stream.read(min(65536, limit + 1 - total))
                            if not data:
                                break
                            total += len(data)
                            if total > limit:
                                raise GateError("greennode: release artifact too large")
                            hashed.update(data)
                        digest = "sha256:" + hashed.hexdigest()
            except OSError as error:
                raise GateError("greennode: unreadable release artifact") from error
            if asset.get("digest") != digest:
                raise GateError("greennode: release artifact digest mismatch")
            if name == "crate-checksums.json":
                try:
                    receipt_data = read_json(receipt_bytes)
                    validate_receipt_data(receipt_data)
                    if receipt_data["tag"] != tag or receipt_data["commit"] != commit:
                        raise ValueError()
                except (ValueError, TypeError, KeyError) as error:
                    raise GateError("greennode: invalid release receipt") from error
    for name in unregistered:
        print(f"{name}: unregistered")


def evidence_file(path, limit):
    path = Path(path)
    if path.is_symlink() or not path.is_file():
        raise GateError("greennode: invalid evidence file")
    with path.open("rb") as stream:
        data = stream.read(limit + 1)
    if len(data) > limit:
        raise GateError("greennode: evidence file too large")
    return data


def bounded_fetch(url, fetch=None):
    try:
        status, data = deadline_fetch(lambda: (fetch or fetch_url)(url), 30)
    except Exception as error:
        raise GateError("greennode: registry lookup failed") from error
    if type(status) is not int or not isinstance(data, bytes) or len(data) > 1024 * 1024:
        raise GateError("greennode: invalid registry response")
    return status, data


def check_owners(names, unregistered, fetch=None):
    if not names or len(set(names)) != len(names) or any(name not in NAMES for name in names):
        raise GateError("greennode: invalid owner arguments")
    for name in names:
        status, data = bounded_fetch(f"https://crates.io/api/v1/crates/{name}/owners", fetch)
        if unregistered:
            if status != 404 or not api_not_found(data, name):
                raise GateError(f"{name}: unregistered name required")
        else:
            payload = read_json(data) if status == 200 else None
            users = payload.get("users") if isinstance(payload, dict) else None
            if (not isinstance(users, list) or not all(isinstance(user, dict) for user in users)
                    or not any(user.get("kind") == "user" and user.get("login") == "dannyota" for user in users)):
                raise GateError(f"{name}: required owner missing")


def previous_release(tag, fetch=None):
    if not isinstance(tag, str) or not TAG.fullmatch(tag) or not valid_semver(tag[1:]):
        raise GateError("greennode: invalid candidate tag")
    status, data = bounded_fetch("https://index.crates.io/gr/ee/greennode", fetch)
    if status != 200 or not data or not data.endswith(b"\n"):
        raise GateError("greennode: invalid registry index")
    seen = set()
    versions = []
    for line in data.split(b"\n")[:-1]:
        row = read_json(line)
        if not index_record_valid(row, "greennode") or row["vers"] in seen:
            raise GateError("greennode: invalid registry index")
        seen.add(row["vers"])
        if VERSION.fullmatch(row["vers"]):
            versions.append((tuple(map(int, row["vers"].split("."))), row))
    if not versions:
        raise GateError("greennode: previous release missing")
    version, row = max(versions, key=lambda item: item[0])
    if version >= tuple(map(int, tag[1:].split("."))):
        raise GateError("greennode: previous release is not below candidate")
    status, data = bounded_fetch(f'https://crates.io/api/v1/crates/greennode/{row["vers"]}', fetch)
    payload = read_json(data) if status == 200 else None
    api = payload.get("version") if isinstance(payload, dict) else None
    if (not isinstance(api, dict) or api.get("crate") != "greennode" or api.get("num") != row["vers"]
            or api.get("checksum") != row["cksum"] or type(api.get("yanked")) is not bool
            or api["yanked"] != row["yanked"]):
        raise GateError("greennode: registry version mismatch")
    return "v" + row["vers"]


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
        if len(args) == 2 and args[0] == "tree":
            tagged_tree(evidence_file(args[1], 8 * 1024 * 1024))
            return 0
        if args and args[0] == "owners":
            unregistered = len(args) > 1 and args[1] == "--unregistered"
            check_owners(args[2:] if unregistered else args[1:], unregistered, fetch)
            return 0
        if len(args) == 2 and args[0] == "previous":
            print(previous_release(args[1], fetch))
            return 0
        if args and args[0] == "evidence" and len(args) in (9, 10):
            publish = len(args) == 10
            if publish and args[9] != "--publish":
                raise GateError("greennode: invalid evidence arguments")
            release = read_json(evidence_file(args[3], 1024 * 1024))
            runs = read_json(evidence_file(args[4], 1024 * 1024))
            owners = {"greennode-sdk": read_json(evidence_file(args[5], 1024 * 1024)),
                      "greennode": read_json(evidence_file(args[6], 1024 * 1024))}
            evidence(args[1], args[2], release, runs, owners, Path(args[7]),
                     evidence_file(args[8], 8 * 1024 * 1024), publish)
            return 0
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
    except (OSError, ValueError, KeyError, TypeError, TimeoutError, TransientError):
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
