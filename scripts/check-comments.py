#!/usr/bin/env -S python3 -I
# Usage: python3 -I scripts/check-comments.py
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ALLOWLIST = Path("scripts/comments-allowlist.txt")
RAW_STRING = re.compile(r'(?:br|cr|r)(#*)"')
ATTRIBUTE = re.compile(r"#\s*!?\s*\[")
DOC_KEY = re.compile(r"(?<![\w$])doc\s*=(?!=)")


def skip_quoted(text, start, quote):
    i, n = start, len(text)
    while i < n and text[i] != quote:
        i += 2 if text[i] == "\\" else 1
    return i + 1


def scan(text):
    comments, literals = [], []
    i, n = 0, len(text)
    while i < n:
        start = i
        if text.startswith("//", i):
            end = text.find("\n", i)
            i = n if end < 0 else end
            comments.append((start, i))
            continue
        if text.startswith("/*", i):
            depth, i = 1, i + 2
            while i < n and depth:
                if text.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif text.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
            comments.append((start, i))
            continue
        raw = RAW_STRING.match(text, i)
        if raw and not (i and (text[i - 1].isalnum() or text[i - 1] == "_")):
            close = '"' + raw.group(1)
            end = text.find(close, raw.end())
            i = n if end < 0 else end + len(close)
            literals.append((start, i))
            continue
        if text[i] == '"':
            i = skip_quoted(text, i + 1, '"')
            literals.append((start, i))
            continue
        if text[i] == "'":
            if text.startswith("\\", i + 1):
                i = skip_quoted(text, i + 1, "'")
                literals.append((start, i))
            elif i + 2 < n and text[i + 2] == "'":
                i += 3
                literals.append((start, i))
            else:
                i += 1
            continue
        i += 1
    return comments, literals


def comment_spans(text):
    return scan(text)[0]


def masked(text, spans):
    chars = list(text)
    for start, end in spans:
        for k in range(start, min(end, len(chars))):
            if chars[k] != "\n":
                chars[k] = " "
    return "".join(chars)


def doc_attribute_positions(code):
    positions = set()
    for attribute in ATTRIBUTE.finditer(code):
        depth, j = 0, attribute.end() - 1
        while j < len(code):
            if code[j] == "[":
                depth += 1
            elif code[j] == "]":
                depth -= 1
                if not depth:
                    break
            j += 1
        for key in DOC_KEY.finditer(code, attribute.end(), j):
            positions.add(key.start())
    return positions


def findings(path, text):
    comments, literals = scan(text)
    lines = text.split("\n")
    found = []
    for start, end in comments:
        fragment = text[start:end]
        sdk_source = path.startswith("crates/greennode-sdk/src/")
        if sdk_source and (fragment.startswith("//!") or
                           (fragment.startswith("///") and not fragment.startswith("////"))):
            continue
        line = text.count("\n", 0, start) + 1
        for offset, fragment in enumerate(text[start:end].split("\n")):
            fragment = fragment.strip()
            if fragment:
                found.append((line + offset, 0, fragment))
    for position in doc_attribute_positions(masked(text, comments + literals)):
        line = text.count("\n", 0, position) + 1
        found.append((line, 1, lines[line - 1].strip()))
    for line, _, fragment in sorted(found):
        yield f"{path}:{line}: {fragment}"


def rust_files():
    listed = subprocess.run(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", "*.rs", "*.inc"],
        cwd=ROOT, check=True, capture_output=True,
    ).stdout
    return sorted({os.fsdecode(name) for name in listed.split(b"\0") if name})


def load_allowlist():
    lines = (ROOT / ALLOWLIST).read_text(encoding="utf-8").splitlines()
    return {line for line in lines if line.strip() and not line.startswith("#")}


def main():
    allowed = load_allowlist()
    found = set()
    violations = []
    for name in rust_files():
        path = ROOT / name
        if not path.is_file():
            continue
        for finding in findings(name, path.read_text(encoding="utf-8")):
            found.add(finding)
            if finding not in allowed:
                violations.append(finding)
    violations.extend(f"{ALLOWLIST}: stale entry: {entry}" for entry in sorted(allowed - found))
    for violation in violations:
        print(violation)
    if violations:
        return 1
    print("check-comments: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
