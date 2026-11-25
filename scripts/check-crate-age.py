#!/usr/bin/env -S python3 -I
# Usage: CRATE_AGE_EXEMPT='name@version reason' python3 -I scripts/check-crate-age.py <base-ref>
import datetime
import json
import os
import subprocess
import sys
import time
import tomllib
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
API = "https://crates.io/api/v1/crates/{}/{}"
USER_AGENT = "crate-age-check"
MIN_AGE_HOURS = 24
MIN_AGE = datetime.timedelta(hours=MIN_AGE_HOURS)
EXEMPT_VAR = "CRATE_AGE_EXEMPT"


def locked(text):
    packages = tomllib.loads(text).get("package", []) if text else []
    return {(p["name"], p["version"]) for p in packages if p.get("source") == CRATES_IO}


def exemptions(value):
    entries = {}
    for line in value.splitlines():
        crate, _, reason = line.strip().partition(" ")
        if not crate:
            continue
        name, _, version = crate.partition("@")
        if not name or not version or not reason.strip():
            raise ValueError(f"{EXEMPT_VAR}: expected 'name@version reason', got: {line.strip()}")
        entries[(name, version)] = reason.strip()
    return entries


def published(name, version):
    url = API.format(urllib.parse.quote(name, safe=""), urllib.parse.quote(version, safe=""))
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=30) as response:
        created = json.load(response)["version"]["created_at"]
    time.sleep(1)
    return datetime.datetime.fromisoformat(created)


def check(base_text, head_text, exempt, fetch, now):
    report, failures = [], 0
    for name, version in sorted(locked(head_text) - locked(base_text)):
        created = fetch(name, version)
        age = now - created
        hours = int(age.total_seconds() // 3600)
        if (name, version) in exempt:
            report.append(f"exempt: {name}@{version} published {hours}h ago: {exempt[(name, version)]}")
        elif age < MIN_AGE:
            report.append(f"too new: {name}@{version} published {hours}h ago, under {MIN_AGE_HOURS}h")
            failures += 1
        else:
            report.append(f"ok: {name}@{version} published {hours}h ago")
    return report, failures


def base_lock(ref):
    commit = subprocess.run(
        ["git", "rev-parse", "--verify", "--end-of-options", f"{ref}^{{commit}}"],
        cwd=ROOT, capture_output=True, text=True,
    )
    if commit.returncode:
        raise ValueError(f"invalid base ref: {ref}")
    shown = subprocess.run(
        ["git", "show", f"{commit.stdout.strip()}:Cargo.lock"], cwd=ROOT, capture_output=True, text=True,
    )
    return shown.stdout if shown.returncode == 0 else ""


def main(argv):
    if len(argv) != 2:
        print("usage: check-crate-age.py <base-ref>", file=sys.stderr)
        return 2
    try:
        exempt = exemptions(os.environ.get(EXEMPT_VAR, ""))
        base = base_lock(argv[1])
        head = (ROOT / "Cargo.lock").read_text(encoding="utf-8")
        now = datetime.datetime.now(datetime.timezone.utc)
        report, failures = check(base, head, exempt, published, now)
    except (OSError, KeyError, ValueError, tomllib.TOMLDecodeError) as error:
        print(f"check-crate-age: {error}", file=sys.stderr)
        return 1
    for line in report:
        print(line)
    if failures:
        print(f"check-crate-age: {failures} crate version(s) under the {MIN_AGE_HOURS} hour cool-down", file=sys.stderr)
        return 1
    print(f"check-crate-age: ok ({len(report)} changed crate versions)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
