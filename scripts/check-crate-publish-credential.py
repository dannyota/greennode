# Usage: python3 -I scripts/check-crate-publish-credential.py --cargo-plugin
import json
import os
import re
import sys
import tomllib
from pathlib import Path

LIMIT = 1048576
TAG = re.compile(r"v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)")
HEX40 = re.compile(r"[0-9a-f]{40}")
HEX64 = re.compile(r"[0-9a-f]{64}")
TOOLCHAIN = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
INDEXES = {"sparse+https://index.crates.io/", "https://github.com/rust-lang/crates.io-index"}


def pairs(items):
    result = {}
    for key, value in items:
        if key in result:
            raise ValueError("duplicate key")
        result[key] = value
    return result


def bounded(path):
    with Path(path).open("rb") as stream:
        data = stream.read(LIMIT + 1)
    if len(data) > LIMIT:
        raise ValueError("input too large")
    return data


def receipt(path):
    data = json.loads(bounded(path), object_pairs_hook=pairs)
    if not isinstance(data, dict) or set(data) != {"tag", "commit", "toolchain", "crates"}:
        raise ValueError("invalid receipt")
    tag, commit, toolchain, crates = (data[key] for key in ("tag", "commit", "toolchain", "crates"))
    if not isinstance(tag, str) or not TAG.fullmatch(tag):
        raise ValueError("invalid tag")
    if not isinstance(commit, str) or not HEX40.fullmatch(commit):
        raise ValueError("invalid commit")
    if not isinstance(toolchain, str) or not TOOLCHAIN.fullmatch(toolchain):
        raise ValueError("invalid toolchain")
    if not isinstance(crates, list) or len(crates) != 2:
        raise ValueError("invalid crates")
    for crate, name in zip(crates, ("greennode-sdk", "greennode")):
        if not isinstance(crate, dict) or set(crate) != {"name", "version", "filename", "sha256"}:
            raise ValueError("invalid crate")
        digest = crate["sha256"]
        if (crate["name"] != name or crate["version"] != tag[1:]
                or crate["filename"] != f"{name}-{tag[1:]}.crate"
                or not isinstance(digest, str) or not HEX64.fullmatch(digest)):
            raise ValueError("invalid crate")
    return crates


def token(env):
    if "CARGO_REGISTRY_TOKEN" in env:
        value = env["CARGO_REGISTRY_TOKEN"]
    else:
        home = env.get("CARGO_HOME")
        if not home:
            raise ValueError("missing cargo home")
        credentials = tomllib.loads(bounded(Path(home) / "credentials.toml").decode("utf-8"))
        value = credentials["registry"]["token"]
    if not isinstance(value, str) or not value:
        raise ValueError("missing token")
    return value


def response(request, env):
    if not isinstance(request, dict) or request.get("v") != 1 or request.get("kind") != "get":
        raise ValueError("unsupported request")
    registry = request.get("registry")
    if not isinstance(registry, dict) or registry.get("index-url") not in INDEXES:
        raise ValueError("unsupported registry")
    operation = request.get("operation")
    if operation == "publish":
        crates = receipt(env["GREENNODE_RELEASE_RECEIPT"])
        if not any(request.get("name") == crate["name"] and request.get("vers") == crate["version"]
                   and request.get("cksum") == crate["sha256"] for crate in crates):
            raise ValueError("unapproved package")
    elif operation != "read":
        raise ValueError("unsupported operation")
    return {"Ok": {"kind": "get", "token": token(env), "cache": "never", "operation_independent": False}}


def main(stdin, stdout, stderr, env):
    print(json.dumps({"v": [1]}), file=stdout, flush=True)
    try:
        line = stdin.readline(LIMIT + 1)
        if len(line) > LIMIT:
            raise ValueError("request too large")
        request = json.loads(line, object_pairs_hook=pairs)
        result = response(request, env)
    except (OSError, ValueError, KeyError, TypeError, UnicodeError):
        result = {"Err": {"kind": "other", "message": "credential request refused"}}
    print(json.dumps(result), file=stdout, flush=True)


if __name__ == "__main__":
    main(sys.stdin, sys.stdout, sys.stderr, os.environ)
