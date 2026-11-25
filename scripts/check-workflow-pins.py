#!/usr/bin/env -S python3 -I
# Usage: GH_TOKEN=<token> python3 -I scripts/check-workflow-pins.py [--list | --tags]
import argparse
import json
import os
import re
import sys
import tomllib
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WORKFLOWS = Path(".github/workflows")
USES_KEY = re.compile(r"""(?<![\w-])["']?uses["']?\s*:""")
EXPLICIT_USES_KEY = re.compile(r"""\s*(?:-\s*)?\?\s*["']?uses\b""")
PIN = re.compile(
    r"\s*(?:-\s+)?uses: ([\w.-]+/[\w.-]+)(?:/[\w./-]+)?@([0-9a-f]{40}) # v([0-9]+\.[0-9]+\.[0-9]+)\s*"
)
NPM_TOOLS = ("prettier", "markdownlint-cli2")
REQUIRED_TOOLS = ("rust", "cargo-deny", "cargo-nextest", "gitleaks", "node", *NPM_TOOLS)


def workflow_files():
    paths = sorted((ROOT / WORKFLOWS).iterdir())
    for path in paths:
        if not path.is_file() or path.suffix not in (".yml", ".yaml"):
            raise ValueError(f"{path.relative_to(ROOT)}: workflows must be .yml or .yaml files")
    return paths


def action_pins(paths):
    pins = {}
    for path in paths:
        name = path.relative_to(ROOT)
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if line.lstrip().startswith("#"):
                continue
            if not (USES_KEY.search(line) or EXPLICIT_USES_KEY.match(line)):
                continue
            match = PIN.fullmatch(line)
            if not match:
                raise ValueError(f"{name}:{number}: expected a block 'uses: owner/repo@<40-hex SHA> # vX.Y.Z'")
            repo, sha, version = match.groups()
            if pins.setdefault(repo.lower(), (repo, sha, version))[1:] != (sha, version):
                raise ValueError(f"{name}:{number}: {repo} has another pin in a workflow")
    return {repo: (sha, version) for repo, sha, version in pins.values()}


def tool_versions():
    pins = {}
    for line in (ROOT / ".tool-versions").read_text(encoding="utf-8").splitlines():
        fields = line.split()
        if fields and not fields[0].startswith("#"):
            if len(fields) != 2 or fields[0] in pins:
                raise ValueError(f".tool-versions: expected one 'name version' line per tool: {line.strip()}")
            pins[fields[0]] = fields[1]
    missing = [tool for tool in REQUIRED_TOOLS if tool not in pins]
    if missing:
        raise ValueError(f".tool-versions: missing {', '.join(missing)}")
    return pins


def pin_mismatches(pins):
    problems = []
    channel = tomllib.loads((ROOT / "rust-toolchain.toml").read_text(encoding="utf-8"))["toolchain"]["channel"]
    if channel != pins["rust"]:
        problems.append(f"rust-toolchain.toml says rust {channel}, .tool-versions says {pins['rust']}")
    declared = json.loads((ROOT / "package.json").read_text(encoding="utf-8")).get("devDependencies", {})
    lock = json.loads((ROOT / "package-lock.json").read_text(encoding="utf-8"))["packages"]
    if lock.get("", {}).get("devDependencies") != declared:
        problems.append("package-lock.json devDependencies differ from package.json")
    for tool in NPM_TOOLS:
        if declared.get(tool) != pins[tool]:
            problems.append(f"package.json says {tool} {declared.get(tool)}, .tool-versions says {pins[tool]}")
        locked = lock.get(f"node_modules/{tool}", {}).get("version")
        if locked != pins[tool]:
            problems.append(f"package-lock.json says {tool} {locked}, .tool-versions says {pins[tool]}")
    return problems


def tag_commit(repo, version):
    request = urllib.request.Request(
        f"https://api.github.com/repos/{repo}/commits/v{version}",
        headers={"Accept": "application/vnd.github+json", "User-Agent": "workflow-pin-check"},
    )
    if os.environ.get("GH_TOKEN"):
        request.add_header("Authorization", f"Bearer {os.environ['GH_TOKEN']}")
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)["sha"]


def tag_mismatches(pins, resolve):
    problems = []
    for repo, (sha, version) in sorted(pins.items()):
        tagged = resolve(repo, version)
        if tagged != sha:
            problems.append(f"{repo} pin {sha} is not v{version} ({tagged})")
    return problems


def main():
    parser = argparse.ArgumentParser()
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--list", action="store_true")
    mode.add_argument("--tags", action="store_true")
    args = parser.parse_args()
    try:
        paths = workflow_files()
        pins = action_pins(paths)
        problems = pin_mismatches(tool_versions())
        if args.tags and not problems:
            problems = tag_mismatches(pins, tag_commit)
    except (OSError, KeyError, UnicodeDecodeError, ValueError, tomllib.TOMLDecodeError) as error:
        print(f"check-workflow-pins: {error}", file=sys.stderr)
        return 1
    for problem in problems:
        print(f"pin mismatch: {problem}", file=sys.stderr)
    if problems:
        return 1
    if args.list:
        for repo, (sha, version) in sorted(pins.items()):
            print(repo, version, sha)
    else:
        checked = "tags of " if args.tags else ""
        print(f"check-workflow-pins: ok ({checked}{len(pins)} actions in {len(paths)} files, tool pins agree)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
