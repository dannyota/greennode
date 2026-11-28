import re
import runpy
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LEXER = runpy.run_path(str(ROOT / "scripts/check-comments.py"))
HELPER = "greennode-sdk/src/wire/product/lossy.rs"
ATTRIBUTE = r"#\s*\[[^\]]*\]\s*"
ATTRIBUTES = rf"(?P<attrs>(?:{ATTRIBUTE})*)"
VISIBILITY = r"(?:pub(?:\s*\([^)]*\))?\s+)?"
ITEM = re.compile(rf"{ATTRIBUTES}{VISIBILITY}\b(?P<kind>struct|enum)\s+\w+\s*")
DERIVE = re.compile(r"#\s*\[\s*derive\s*\([^)]*\bDeserialize\b[^)]*\)")
FIELD = re.compile(
    rf"\s*{ATTRIBUTES}{VISIBILITY}(?:(?:r\#)?\w+\s*:(?!:)\s*)?(?P<type>\S.*?)\s*", re.S
)
VARIANT = re.compile(rf"\s*(?:{ATTRIBUTE})*(?:r\#)?\w+\s*(?=[{{(])")
TYPE_ALIAS = re.compile(r"\btype\s+(\w+)\s*(?:<[^;=]*?>\s*)?=\s*([^;]+);")
IDENTIFIER = re.compile(r"\b\w+\b")
SERDE = re.compile(r"#\s*\[\s*serde\s*\(")
WITH = re.compile(
    r'\s*deserialize_with\s*=\s*"(?:crate::wire::product::)?lossy::'
    r'(?:boxed|optional_boxed|present_boxed|borrowed|optional_borrowed|borrowed_map)"\s*'
)
DELIMITER = re.compile(r"->|[<>{}\[\](),;]")
CLOSING = {"<": ">", "{": "}", "[": "]", "(": ")"}


def item_body(code, start):
    pending = []
    for token in DELIMITER.finditer(code, start):
        symbol = token[0]
        if not pending:
            if symbol == ";":
                return None
            if symbol in "{(":
                return token.start()
        if symbol in CLOSING:
            pending.append(CLOSING[symbol])
        elif pending and symbol == pending[-1]:
            pending.pop()
    return None


def entries(code, opening):
    pending = [CLOSING[code[opening]]]
    start = opening + 1
    for token in DELIMITER.finditer(code, start):
        symbol = token[0]
        if len(pending) == 1 and symbol in (",", pending[-1]):
            yield start, token.start()
            start = token.end()
        if symbol in CLOSING:
            pending.append(CLOSING[symbol])
        elif symbol == pending[-1]:
            pending.pop()
            if not pending:
                break


def field_ranges(code, item):
    opening = item_body(code, item.end())
    if opening is None:
        return
    if item["kind"] == "struct":
        yield from entries(code, opening)
        return
    for start, end in entries(code, opening):
        variant = VARIANT.match(code, start, end)
        if variant:
            yield from entries(code, variant.end())


def mask(source):
    comments, literals = LEXER["scan"](source)
    without_comments = LEXER["masked"](source, comments)
    return without_comments, LEXER["masked"](without_comments, literals)


def raw_names(sources):
    aliases = [
        (match[1], set(IDENTIFIER.findall(match[2])))
        for code in sources
        for match in TYPE_ALIAS.finditer(code)
    ]
    names = {"RawValue"}
    while True:
        added = {alias for alias, targets in aliases if not names.isdisjoint(targets)} - names
        if not added:
            return names
        names.update(added)


def checked_attribute(code, source, start, end):
    for attribute in SERDE.finditer(code, start, end):
        for arg_start, arg_end in entries(code, attribute.end() - 1):
            if WITH.fullmatch(source, arg_start, arg_end):
                return True
    return False


def check(path, without_comments, code, names):
    if path == HELPER:
        return []
    lines = []
    for item in ITEM.finditer(code):
        if not DERIVE.search(item["attrs"]):
            continue
        for start, end in field_ranges(code, item):
            field = FIELD.fullmatch(code, start, end)
            if field is None or names.isdisjoint(IDENTIFIER.findall(field["type"])):
                continue
            if not checked_attribute(code, without_comments, *field.span("attrs")):
                lines.append(code.count("\n", 0, field.start("type")) + 1)
    return lines


def violations(path, source):
    without_comments, code = mask(source)
    return check(path, without_comments, code, raw_names([code]))


def audit(workspace):
    crates = workspace / "crates"
    sources = [
        (file.relative_to(crates).as_posix(), *mask(file.read_text()))
        for root in sorted(crates.glob("*/src"))
        for file in sorted(root.rglob("*.rs"))
    ]
    names = raw_names(code for _, _, code in sources)
    return [
        f"{path}:{line}: deserialize RawValue through wire::product::lossy"
        for path, without_comments, code in sources
        for line in check(path, without_comments, code, names)
    ]


if __name__ == "__main__":
    errors = audit(ROOT)
    if errors:
        sys.exit("\n".join(errors))
    print("check-raw-values: ok")
