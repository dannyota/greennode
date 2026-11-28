import re
import runpy
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LEXER = runpy.run_path(str(ROOT / "scripts/check-comments.py"))

STATUS = r"(?:404(?:_?u(?:16|32|64|size))?|StatusCode\s*::\s*NOT_FOUND)"
NOT_FOUND = r"(?:\w+\s*::\s*)*ErrorClass\s*::\s*NotFound"
CANDIDATE = re.compile(
    rf"\bStatusCode\s*::\s*NOT_FOUND\b|\bSome\s*\(\s*{STATUS}\s*,?\s*\)"
    rf"|(?:==|!=)\s*(?:{STATUS}|{NOT_FOUND})\b|\b(?:{STATUS}|{NOT_FOUND})\s*(?:==|!=|=>)"
    rf"|\bhas_(?:class|identity)\s*\(\s*{NOT_FOUND}\s*,?\s*\)|\b{STATUS}\b"
)
MATCHES_START = re.compile(r"\bmatches\s*!\s*\(")
NOT_FOUND_PATTERN = re.compile(rf"\b{NOT_FOUND}\b")
PATTERN_DECISION = re.compile(r"\b(?:if|while)\s+let\s+[^;{}=]*=|\([^;{}=]*\)\s*=>")
RAW_SEND = re.compile(
    r"\basync\s+fn\s+(?:send_\w+|execute_raw)\b[^{}]*?->\s*[^{}]*?\bResult\s*<\s*"
    r"(?:\(\s*StatusCode\s*,\s*)?(?:bytes\s*::\s*)?Bytes\b"
)
RECEIPT_SENDERS = {
    "loadbalancer/child_writes.rs": {"send": "Response", "send_once": "Response"},
    "loadbalancer/write.rs": {"send_once": "Response"},
    "loadbalancer/child_deletes.rs": {"send_child_delete": "Response"},
    "loadbalancer/deletes.rs": {"send_delete": "Response"},
    "compute/mutation.rs": {"send_mutation": "Response"},
    "containerregistry/mod.rs": {"fallback_inventory": "Guarded"},
}
RECEIPT_CONSUMERS = {
    "database/write.rs": {"poll_instance": ("target", "ConfirmationTarget")},
    "containerregistry/repository_writes.rs": {"wait_repository": ("target", "ConfirmationTarget")},
    "iam/policy_writes.rs": {"confirm_created_policy": ("receipt", "AcknowledgedId")},
    "loadbalancer/child_writes.rs": {"acknowledged_id": ("reply", "Response")},
    "loadbalancer/write.rs": {"from_receipt": ("receipt", "CleanReceipt")},
    "loadbalancer/wire.rs": {"pool_in_use_refusal": ("receipt", "CleanReceipt")},
    "compute/server_writes.rs": {"create_id": ("reply", "Response")},
    "containerregistry/reads.rs": {"complete_absence": ("receipt", "Guarded")},
    "wire/product/response.rs": {"inspect_error_message": ("receipt", "CleanReceipt")},
}
DISCARD_POLICIES = {
    "compute/mutation.rs": ("send_mutation", r"!matches!\s*\(\s*mutation\.id"),
    "containerregistry/user_writes.rs": ("delete_user", r"true"),
    "globalloadbalancer/write.rs": ("write_operation", r"method\s*!=\s*Method::POST"),
    "loadbalancer/child_deletes.rs": ("send_child_delete", r"true"),
    "loadbalancer/child_writes.rs": ("send_once", r"\*method\s*!=\s*Method::POST"),
    "loadbalancer/deletes.rs": ("send_delete", r"true"),
    "volume/write.rs": ("send_write", r"mutation\.method\s*!=\s*Method::POST"),
}
QUALIFIED_TYPE = r"(?:\w+\s*::\s*)*"
SDK_PATH = "greennode-sdk/src/"

TEST_ITEM = re.compile(
    r"#\s*\[\s*(?:cfg\s*\(\s*test\s*\)|(?:tokio\s*::\s*)?test(?:\([^\]]*\))?)\s*\]"
    r"\s*(?:#\s*\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:mod|fn)\s+\w+"
)
ALLOW = {
    "greennode-sdk/src/cdn/wire.rs": (
        "404 => ErrorClass::NotFound,",
        "if self.not_found.is_some() && class == ErrorClass::NotFound { return absent(error); }",
    ),
    "greennode-sdk/src/network/endpoint_quote.rs": (
        'ErrorClass::NotFound => ("NotFound", format!("vngcloud: resource not found: {OP}{detail}"),),',
    ),
    "greennode-sdk/src/network/nat_quote.rs": (
        'ErrorClass::NotFound => format!("vngcloud: resource not found: {OP}{separator}{detail}"),',
    ),
    "greennode-sdk/src/compute/wire.rs": ('Some(404) => "NotFound",',),
    "greennode-sdk/src/error.rs": (
        "matches!(class, ErrorClass::Auth | ErrorClass::NotFound | ErrorClass::PermissionDenied | ErrorClass::RateLimited)",
    ),
    "greennode-sdk/src/globalloadbalancer/write.rs": (
        "matches!(error.class(), ErrorClass::Auth | ErrorClass::NotFound | ErrorClass::PermissionDenied | ErrorClass::RateLimited)",
    ),
    "greennode-sdk/src/error/provenance.rs": (
        "404 => Some(ErrorClass::NotFound),",
        'Some(404) => "NotFound",',
    ),
    "greennode-sdk/src/quote/catalog/storage.rs": ("Some(404) => Some(ErrorClass::NotFound),",),
    "greennode-sdk/src/monitor/wire.rs": ("&[StatusCode::OK, StatusCode::NOT_FOUND]",),
    "greennode-sdk/src/monitor/quota/wire.rs": (
        'read.error(Some(404), "NotFound", "quota response had invalid structure", false, &self.evidence,)',
    ),
    "greennode-sdk/src/monitor/quota/quotes.rs": (
        'reader.read.error(Some(404), "NotFound", "quota response withheld", false, &reader.evidence,)',
    ),
    "greennode/src/cli/errors.rs": ("let not_found = error.has_class(ErrorClass::NotFound);",),
}
PROOF_MODULE = "greennode-sdk/src/wire/product/absence.rs"


def test_spans(code):
    for item in TEST_ITEM.finditer(code):
        delimiter = re.search(r"[{;]", code[item.end():])
        if delimiter is None:
            continue
        end = item.end() + delimiter.end()
        if delimiter[0] == "{":
            depth = 1
            for brace in re.finditer(r"[{}]", code[end:]):
                depth += 1 if brace[0] == "{" else -1
                if depth == 0:
                    end += brace.end()
                    break
        yield item.start(), end


def matches_arms(code):
    for token in MATCHES_START.finditer(code):
        depth = 1
        arm_start = None
        for delimiter in re.finditer(r"[()[\]{},]", code[token.end():]):
            if delimiter[0] in "([{":
                depth += 1
            elif delimiter[0] in ")]}":
                depth -= 1
            elif depth == 1 and arm_start is None:
                arm_start = token.end() + delimiter.end()
            if depth == 0:
                if arm_start is not None:
                    yield from NOT_FOUND_PATTERN.finditer(code, arm_start, token.end() + delimiter.start())
                break


def signature_parts(code, name):
    start = re.search(rf"\bfn\s+{name}\b\s*(?:<[^(){{]*>)?\s*\(", code)
    if start is None:
        return 0, "", ""
    depth = 1
    for delimiter in re.finditer(r"[()]", code[start.end():]):
        depth += 1 if delimiter[0] == "(" else -1
        if depth == 0:
            params_end = start.end() + delimiter.start()
            result = code[params_end + 1:].split("{", 1)[0]
            return start.start(), code[start.end():params_end], result
    return start.start(), "", ""


def balanced_index(code, start, opening, closing):
    depth = 0
    for i in range(start, len(code)):
        if code[i] == opening:
            depth += 1
        elif code[i] == closing:
            depth -= 1
            if depth == 0:
                return i
    return len(code)


def definition_body(code, name):
    signature = re.search(rf"\bfn\s+{name}\b[^{{;]*\{{", code)
    if signature is None:
        return 0, ""
    start = signature.end() - 1
    return start, code[start:balanced_index(code, start, "{", "}")]


def enclosing_function(code, position):
    starts = list(re.finditer(r"\bfn\s+\w+\b[^{};]*\{", code[:position]))
    for signature in reversed(starts):
        opening = signature.end() - 1
        closing = balanced_index(code, opening, "{", "}")
        if position < closing:
            return opening + 1, closing
    return position, len(code)


def block_lineage(code, scope_start, position):
    openings = []
    for brace in re.compile(r"[{}]").finditer(code, scope_start, position):
        if brace[0] == "{":
            openings.append(brace.start())
        elif openings:
            openings.pop()
    return openings


def visible_binding(code, scope_start, position, name):
    lineage = block_lineage(code, scope_start, position)
    declarations = list(re.compile(rf"\b(?:let|const)\s+(?:mut\s+)?{name}\b").finditer(code, scope_start, position))
    for binding in reversed(declarations):
        scope = block_lineage(code, scope_start, binding.start())
        if lineage[:len(scope)] == scope:
            return binding
    return None


def literal_discards(code, start):
    end = balanced_index(code, start, "{", "}")
    return re.search(r"\bno_output\s*:\s*true\b", code[start:end]) is not None


def declares_discard(code, call, end, scope_start):
    arguments = code[call.end():end]
    for inline in re.finditer(r"\bOperation\s*\{", arguments):
        start = inline.end() - 1
        if literal_discards(arguments, start):
            return True
    for reference in re.finditer(r"&\s*(\w+)\b", arguments):
        name = reference[1]
        binding = visible_binding(code, scope_start, call.start(), name)
        if not binding:
            continue
        operation = rf"{QUALIFIED_TYPE}Operation\s*\{{"
        declaration = re.compile(rf"(?:\s*:\s*{QUALIFIED_TYPE}Operation)?\s*=\s*{operation}")
        initial = declaration.match(code, binding.end(), call.start())
        discarded = bool(initial and literal_discards(code, initial.end() - 1))
        changes = re.compile(rf"(?<![.\w:]){name}\s*(?P<field>\.\s*no_output\s*)?=(?!=|>)\s*")
        call_scope = block_lineage(code, scope_start, call.start())
        assignments_start = binding.end() if not initial else balanced_index(code, initial.end() - 1, "{", "}") + 1
        for change in changes.finditer(code, assignments_start, call.start()):
            target = visible_binding(code, scope_start, change.end(), name)
            if not target or target.start() != binding.start():
                continue
            replacement = re.compile(operation).match(code, change.end(), call.start())
            if change["field"] is not None or not replacement or not literal_discards(code, replacement.end() - 1):
                discarded = False
            else:
                change_scope = block_lineage(code, scope_start, change.start())
                if call_scope[:len(change_scope)] == change_scope:
                    discarded = True
        if discarded:
            return True
    return False


def discarded_raw_calls(code):
    for call in re.finditer(r"\b(?:operation\s*::\s*)?execute_raw\s*\(", code):
        scope_start, scope_end = enclosing_function(code, call.start())
        end = balanced_index(code, call.end() - 1, "(", ")")
        tail = code[end + 1:scope_end]
        awaited = re.match(r"\s*\.await\b", tail)
        if not awaited or declares_discard(code, call, end, scope_start):
            continue
        tail = tail[awaited.end():]
        while mapping := re.match(r"\s*\.map_err\s*\(", tail):
            tail = tail[balanced_index(tail, mapping.end() - 1, "(", ")") + 1:]
        if re.match(r"\s*\.map\s*\(\s*(?:(?:std::mem::)?drop|\|_\|\s*\(\))\s*\)", tail):
            yield call.start()
        elif await_end := re.match(r"\s*\??\s*;", tail):
            start = max(code.rfind(separator, scope_start - 1, call.start()) for separator in (";", "{", "}")) + 1
            prefix = code[max(start, scope_start):call.start()].strip()
            if not prefix:
                yield call.start()
            elif binding := re.fullmatch(r"let\s+(?:mut\s+)?(\w+)\s*=", prefix):
                name = binding[1]
                subsequent = tail[await_end.end():]
                subsequent_start = scope_end - len(subsequent)
                original = visible_binding(code, scope_start, call.start(), name)
                dropped = re.match(rf"\s*(?:std\s*::\s*mem\s*::\s*)?drop\s*\(\s*{name}\s*\)", subsequent)
                uses = re.compile(rf"\b{name}\s*\.\s*(?:check_discarded_acknowledgement|clean_receipt|decode_projection)\s*\(").finditer(code, subsequent_start, scope_end)
                checked = original and any(
                    (target := visible_binding(code, scope_start, use.start(), name)) and target.start() == original.start()
                    for use in uses
                )
                if name == "_" or dropped or (name.startswith("_") and not checked):
                    yield call.start()


def boundary_violations(file, code):
    if file == "wire/product/operation.rs":
        for name, check in [("send_route", "check_write_acknowledgement"), ("decode_response", "check_discarded_acknowledgement")]:
            start, body = definition_body(code, name)
            check_at = body.find(f".{check}(")
            early = re.search(r"\breturn\s+Ok\s*\(", body)
            if check_at < 0 or (early and early.start() < check_at):
                yield start
    else:
        for call in re.finditer(r"\bsender\s*::\s*run\s*\(", code):
            yield call.start()

    if policy := DISCARD_POLICIES.get(file):
        name, value = policy
        start, body = definition_body(code, name)
        if not re.search(rf"\bno_output\s*:\s*{value}", body):
            yield start

    for ack in re.finditer(r"\b(?:struct|enum)\s+(?:CreateAck\w*|CreatedId|IssuedAccount|\w*Acknowledgement)\b[^;{]*(?=[;{])", code):
        opening = code[ack.end()]
        if opening == "{":
            body = code[ack.end():balanced_index(code, ack.end(), "{", "}")]
        else:
            body = ack[0]
        if re.search(r"\bString\b", body):
            yield ack.start()

    ack_registrations = {
        "loadbalancer/child_writes.rs": ("acknowledged_id", "keep_acknowledgement"),
        "globalloadbalancer/child/mod.rs": ("acceptance", "register_acknowledgement"),
    }
    if registration := ack_registrations.get(file):
        name, call = registration
        start, body = definition_body(code, name)
        if not re.search(rf"\.\s*{call}\s*\(", body):
            yield start
    for bypass in re.finditer(r"\backnowledgement\s*:\s*Option\s*<\s*Guarded\b", code):
        yield bypass.start()

    protected_reads = {"network/vpc/wire.rs": "request", "network/route_inventory.rs": "read_route_inventory"}
    if name := protected_reads.get(file):
        start, body = definition_body(code, name)
        for call in re.finditer(r"\bread\s*\.\s*(?:api|vpn_refused)\s*\(", body):
            end = balanced_index(body, call.end() - 1, "(", ")")
            if not re.match(r"\s*\.\s*retaining_response_proof\s*\(", body[end + 1:]):
                yield start + call.start()


def receipt_violations(path, code):
    for raw in RAW_SEND.finditer(code):
        yield raw.start()
    if not path.startswith(SDK_PATH):
        return
    file = path.removeprefix(SDK_PATH)
    yield from boundary_violations(file, code)
    yield from discarded_raw_calls(code)
    for name, receipt in RECEIPT_SENDERS.get(file, {}).items():
        start, _, result = signature_parts(code, name)
        if not re.match(rf"\s*->\s*{QUALIFIED_TYPE}Result\s*<\s*{QUALIFIED_TYPE}{receipt}\b", result):
            yield start
    for name, (parameter, receipt) in RECEIPT_CONSUMERS.get(file, {}).items():
        start, params, _ = signature_parts(code, name)
        pattern = rf"(?:^|,)\s*{parameter}\s*:\s*(?:&\s*)?{QUALIFIED_TYPE}{receipt}\b"
        if not re.search(pattern, params):
            yield start


def violations(path, source):
    if path == PROOF_MODULE:
        return []
    comments, literals = LEXER["scan"](source)
    masked = LEXER["masked"]
    without_comments = masked(source, comments)
    code = masked(without_comments, literals)
    code = masked(code, test_spans(code))
    allowed = []
    for expression in ALLOW.get(path, ()):
        tokens = re.findall(r'"[^"\n]*"|\w+|::|=>|[^\s]', expression)
        pattern = r"\s*".join(map(re.escape, tokens))
        allowed.extend(match.span() for match in re.finditer(pattern, without_comments))
    candidates = [*CANDIDATE.finditer(code), *matches_arms(code)]
    for pattern in PATTERN_DECISION.finditer(code):
        candidates.extend(NOT_FOUND_PATTERN.finditer(code, pattern.start(), pattern.end()))
    starts = set(receipt_violations(path, code))
    for match in candidates:
        if not any(start <= match.start() and match.end() <= end for start, end in allowed):
            starts.add(match.start())
    return [source.count("\n", 0, start) + 1 for start in sorted(starts)]


def audit(workspace):
    crates = workspace / "crates"
    results = []
    for root in sorted(crates.glob("*/src")):
        for file in sorted(root.rglob("*.rs")):
            path = file.relative_to(crates)
            if "tests" in path.parts or file.name.endswith(("tests.rs", "_test.rs")):
                continue
            results.extend(
                f"{path}:{line}: use a clean request-bound receipt for response decisions"
                for line in violations(path.as_posix(), file.read_text())
            )
    return results


if __name__ == "__main__":
    errors = audit(ROOT)
    if errors:
        sys.exit("\n".join(errors))
    print("check-absence: ok")
