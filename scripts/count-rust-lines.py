#!/usr/bin/env -S python3 -I
# Usage: python3 -I scripts/count-rust-lines.py <file.rs>
import importlib.util
import re
import sys
from pathlib import Path

sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location(
    "check_comments", Path(__file__).resolve().with_name("check-comments.py")
)
lexer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(lexer)

TEST_ATTRIBUTE = re.compile(r"#\[cfg\(test\)\]\s*")
MODULE = re.compile(r"(?:pub(?:\s*\(crate\))?\s+)?mod\s+\w+\s*\{\s*")
EMPTY_MODULE = re.compile(r"(?:pub(?:\s*\(crate\))?\s+)?mod\s+\w+\s*\{\s*\}\s*")


def counted_lines(text):
    comments, literals = lexer.scan(text)
    code = lexer.masked(text, comments + literals)
    raw = [line.removesuffix("\r") for line in text.split("\n")]
    if raw and raw[-1] == "":
        raw.pop()
    starts, depths, depth = [], [], 0
    offset = 0
    for line in code.split("\n")[: len(raw)]:
        starts.append(offset)
        depths.append(depth)
        depth += line.count("{") - line.count("}")
        offset += len(line) + 1
    lines = [line.removesuffix("\r") for line in code.split("\n")[: len(raw)]]
    excluded = 0
    i = 0
    while i + 1 < len(raw):
        if depths[i] == 0 and TEST_ATTRIBUTE.fullmatch(lines[i]) and raw[i] == lines[i]:
            if EMPTY_MODULE.fullmatch(lines[i + 1]):
                excluded += 2
                i += 2
                continue
            if MODULE.fullmatch(lines[i + 1]):
                close = closing_line(code, starts, starts[i + 1] + lines[i + 1].index("{"))
                if close is not None and raw[close].rstrip() == "}":
                    excluded += close - i + 1
                    i = close + 1
                    continue
        i += 1
    return len(raw) - excluded


def closing_line(code, starts, brace):
    depth = 0
    for position in range(brace, len(code)):
        if code[position] == "{":
            depth += 1
        elif code[position] == "}":
            depth -= 1
            if not depth:
                line = code.count("\n", 0, position)
                return line if position == starts[line] else None
    return None


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: count-rust-lines.py <file.rs>")
    print(counted_lines(Path(sys.argv[1]).read_text(encoding="utf-8")))
