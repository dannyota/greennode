import runpy
import tempfile
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1]
GATE = runpy.run_path(str(SCRIPTS / "check-absence.py"))


class AbsenceAuditTests(unittest.TestCase):
    def test_multiline_expressions_and_comments_do_not_hide_status_checks(self):
        source = '''
if error.http_status() ==
    Some(/* ignored */ 404,
    ) {}
if error.has_identity(
    crate::ErrorClass::NotFound
) {}
if 404_u16
    == status {}
match status { http::StatusCode :: NOT_FOUND => return Ok(()) }
if matches!(error.class(),
    ErrorClass::NotFound
) {}
if status == StatusCode::from_u16(404).unwrap() {}
'''
        self.assertEqual(len(GATE["violations"]("arbitrary/src/writes.rs", source)), 6)

    def test_if_let_and_tuple_arms_cannot_certify_absence(self):
        source = """
if let crate::ErrorClass::NotFound = error.class() {}
match (operation, error.class()) {
    ("delete", crate::ErrorClass::NotFound) => true,
    _ => false,
}
"""
        self.assertEqual(len(GATE["violations"]("arbitrary/src/writes.rs", source)), 2)

    def test_rust_literals_and_test_items_do_not_count(self):
        source = '''
const RULE: &str = r#"if status == 404 or class == ErrorClass::NotFound"#;
// if status == 404
#[cfg(test)]
mod tests { fn example() { if status == 404 {} } }
#[tokio::test]
async fn check() { if error.has_class(ErrorClass::NotFound) {} }
'''
        self.assertEqual(GATE["violations"]("arbitrary/src/writes.rs", source), [])

    def test_allowlist_is_scoped_to_the_same_expression_and_path(self):
        source = 'match status { Some(404) => "NotFound", Some(404) => return Ok(()), }'
        self.assertEqual(GATE["violations"]("greennode-sdk/src/compute/wire.rs", source), [1])
        self.assertEqual(GATE["violations"]("greennode/src/compute/wire.rs", source), [1, 1])

    def test_raw_send_cannot_return_detached_status_and_bytes(self):
        bad = "async fn send_once() -> Result<(StatusCode, Bytes), Failure<()>> {}"
        good = "async fn send_once() -> Result<Response, Failure<()>> {}"
        self.assertEqual(GATE["violations"]("arbitrary/src/write.rs", bad), [1])
        self.assertEqual(GATE["violations"]("arbitrary/src/write.rs", good), [])

    def test_busy_and_inventory_classifiers_require_receipt_types(self):
        for path, name, receipt in [
            ("wire/product/response.rs", "inspect_error_message", "CleanReceipt<'_>"),
            ("containerregistry/reads.rs", "complete_absence", "Guarded<Inventory>"),
        ]:
            path = f"greennode-sdk/src/{path}"
            self.assertEqual(GATE["violations"](path, f"fn {name}(body: &[u8]) {{}}"), [1])
            self.assertEqual(GATE["violations"](path, f"fn {name}(receipt: {receipt}) {{}}"), [])

    def test_receipt_type_must_appear_in_the_checked_signature_position(self):
        sender = "greennode-sdk/src/compute/mutation.rs"
        consumer = "greennode-sdk/src/compute/server_writes.rs"
        policy = "{ let op = Operation { no_output: !matches!(mutation.id, code) }; }"
        self.assertEqual(GATE["violations"](sender, "fn send_mutation(input: Response) -> Result<Bytes, Error> " + policy), [1])
        self.assertEqual(GATE["violations"](sender, "fn send_mutation(input: Bytes) -> Result<sender::Response, Error> " + policy), [])
        self.assertEqual(GATE["violations"](consumer, "fn create_id(reply: &[u8]) -> Response {}"), [1])
        self.assertEqual(GATE["violations"](consumer, "fn create_id(reply: &other::Response) -> Result<String, Error> {}"), [])

    def test_write_success_cannot_bypass_the_shared_screen(self):
        bad = "fn send_route() { return Ok(reply); reply.check_write_acknowledgement(op)?; }"
        good = "fn send_route() { reply.check_write_acknowledgement(op)?; Ok(reply) }"
        discard = "fn decode_response() { response.check_discarded_acknowledgement(op)?; Ok(()) }"
        path = "greennode-sdk/src/wire/product/operation.rs"
        self.assertTrue(GATE["violations"](path, bad + discard))
        self.assertEqual(GATE["violations"](path, good + discard), [])
        self.assertTrue(GATE["violations"]("greennode-sdk/src/other.rs", "sender::run()"))

    def test_raw_discarders_declare_their_write_policy(self):
        path = "greennode-sdk/src/containerregistry/user_writes.rs"
        self.assertEqual(GATE["violations"](path, "fn delete_user() { Operation { no_output: false } }"), [1])
        self.assertEqual(GATE["violations"](path, "fn delete_user() { Operation { no_output: true } }"), [])

    def test_undeclared_write_discard_cannot_drop_the_raw_response(self):
        path = "greennode-sdk/src/new_service.rs"
        for discard in ("", "let _reply = "):
            source = f"""async fn write() {{
    let op = Operation {{ class: Write, no_output: false }};
    {discard}operation::execute_raw(&op).await?;
    Ok(())
}}"""
            self.assertEqual(GATE["violations"](path, source), [3])
        dropped = """async fn write() {
    let op = Operation { class: Write, no_output: false };
    let reply = operation::execute_raw(&op).await?;
    drop(reply);
    Ok(())
}"""
        self.assertEqual(GATE["violations"](path, dropped), [3])
        source = """async fn write() {
    let op = Operation { class: Write, no_output: true };
    let reply = operation::execute_raw(&op).await?;
    reply.check_discarded_acknowledgement(op.id)
}"""
        self.assertEqual(GATE["violations"](path, source), [])

    def test_discard_policy_survives_error_mapping(self):
        path = "greennode-sdk/src/new_service.rs"
        for value, expected in [("true", []), ("false", [3])]:
            source = f"""async fn write() {{
    let op = Operation {{ class: Write, no_output: {value} }};
    operation::execute_raw(&op).await.map_err(|error| wrap(error))?;
    Ok(())
}}"""
            self.assertEqual(GATE["violations"](path, source), expected)

    def test_discard_declarations_do_not_cross_function_boundaries(self):
        path = "greennode-sdk/src/new_service.rs"
        source = """fn first() { let op = Operation { no_output: true }; }
async fn second(op: Operation) {
    operation::execute_raw(&op).await?;
    Ok(())
}"""
        self.assertEqual(GATE["violations"](path, source), [3])

    def test_named_underscore_binding_can_be_checked(self):
        path = "greennode-sdk/src/new_service.rs"
        checked = """async fn write(op: Operation) {
    let _reply = operation::execute_raw(&op).await?;
    _reply.check_discarded_acknowledgement(op.id)
}"""
        self.assertEqual(GATE["violations"](path, checked), [])
        wildcard = """async fn write(op: Operation) {
    let _ = operation::execute_raw(&op).await?;
    Ok(())
}"""
        self.assertEqual(GATE["violations"](path, wildcard), [2])

    def test_mutable_operation_can_declare_a_discard(self):
        path = "greennode-sdk/src/new_service.rs"
        source = """async fn write() {
    let mut op = Operation { no_output: true };
    op.retry = Once;
    operation::execute_raw(&op).await?;
    Ok(())
}"""
        self.assertEqual(GATE["violations"](path, source), [])

    def test_inner_operation_does_not_change_the_outer_discard_policy(self):
        path = "greennode-sdk/src/new_service.rs"
        for outer, inner, expected in [("false", "true", [6]), ("true", "false", [])]:
            source = f"""async fn write() {{
    let op = Operation {{ no_output: {outer} }};
    {{
        let op = Operation {{ no_output: {inner} }};
    }}
    operation::execute_raw(&op).await?;
    Ok(())
}}"""
            self.assertEqual(GATE["violations"](path, source), expected)

    def test_discard_policy_follows_whole_operation_reassignment(self):
        path = "greennode-sdk/src/new_service.rs"
        for initial, assigned, expected in [("true", "false", [4]), ("false", "true", [])]:
            source = f"""async fn write() {{
    let mut op = Operation {{ no_output: {initial} }};
    op = Operation {{ no_output: {assigned} }};
    let _ = operation::execute_raw(&op).await?;
    Ok(())
}}"""
            self.assertEqual(GATE["violations"](path, source), expected)

    def test_reassignment_respects_shadowing_and_conditional_blocks(self):
        path = "greennode-sdk/src/new_service.rs"
        for inner, expected in [
            ("if choice { op = Operation { no_output: false }; }", [4]),
            ("{ let mut op = other(); op = Operation { no_output: false }; }", []),
            ("op = other();", [4]),
            ("let _same = op.no_output == false;", []),
        ]:
            source = f"""async fn write() {{
    let mut op = Operation {{ no_output: true }};
    {inner}
    let _ = operation::execute_raw(&op).await?;
    Ok(())
}}"""
            self.assertEqual(GATE["violations"](path, source), expected)

    def test_inner_binding_does_not_excuse_or_hide_an_outer_receipt(self):
        path = "greennode-sdk/src/new_service.rs"
        source = """async fn write(op: Operation) {
    let _reply = operation::execute_raw(&op).await?;
    {
        let _reply = other();
        _reply.check_discarded_acknowledgement(op.id)?;
    }
    %s
}"""
        self.assertEqual(GATE["violations"](path, source % "Ok(())"), [2])
        self.assertEqual(GATE["violations"](path, source % "_reply.check_discarded_acknowledgement(op.id)"), [])

    def test_acknowledgements_cannot_store_bare_string_ids(self):
        path = "greennode-sdk/src/iam/user_writes.rs"
        self.assertEqual(GATE["violations"](path, "enum CreateAck { Object(String) }"), [1])
        self.assertEqual(GATE["violations"](path, "enum CreateAck { Object(AcknowledgedId) }"), [])
        self.assertEqual(GATE["violations"](path, "struct CreateAck(String);"), [1])

    def test_guarded_acknowledgement_must_register_before_a_later_request(self):
        path = "greennode-sdk/src/globalloadbalancer/child/mod.rs"
        bad = "fn acceptance() { let row = reply.guard(value); Some(row) }"
        good = "fn acceptance() { let row = reply.guard(value); row.register_acknowledgement(evidence, select)?; Some(row) }"
        self.assertEqual(GATE["violations"](path, bad), [1])
        self.assertEqual(GATE["violations"](path, good), [])

    def test_protected_error_translation_keeps_the_classified_proof(self):
        path = "greennode-sdk/src/network/route_inventory.rs"
        bad = "fn read_route_inventory() { read.api(status, code) }"
        good = "fn read_route_inventory() { read.api(status, code).retaining_response_proof(&error) }"
        self.assertEqual(GATE["violations"](path, bad), [1])
        self.assertEqual(GATE["violations"](path, good), [])

    def test_workspace_gate_scans_every_crate(self):
        with tempfile.TemporaryDirectory() as tmp:
            workspace = Path(tmp)
            for crate in ("greennode-sdk", "greennode", "other-workspace-member"):
                path = workspace / "crates" / crate / "src" / "lib.rs"
                path.parent.mkdir(parents=True)
                path.write_text("fn example() { if status == 404 {} }")
            self.assertEqual(len(GATE["audit"](workspace)), 3)


if __name__ == "__main__":
    unittest.main()
