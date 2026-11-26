use super::{client, response};
use crate::iam::{
    AttachUserPolicyInput, CreatePolicyInput, DeletePolicyInput, Statement, UpdatePolicyInput,
    WriteError,
};
use crate::wire::test_server::{Response, Server};
use crate::{ErrorClass, RequestContext, WriteDispatch};
use serde_json::{Value, json};

const BEARER: &str = "toy/bearer";
const POLICY_ID: &str = "pol-1";

fn caller() -> Response {
    response("200 OK", br#"{"userId":"usr-1","userType":"iam-user"}"#)
}

fn catalog() -> Response {
    response(
        "200 OK",
        br#"[{"action":"CreatePolicy","label":"Write","resources":[]}]"#,
    )
}

fn input() -> CreatePolicyInput {
    let mut statement = Statement::default();
    statement.effect = "allow".into();
    statement.actions = vec!["iam:ListActions".into()];
    statement.resources = vec!["*".into()];
    CreatePolicyInput::new("reader", vec![statement])
}

#[tokio::test]
async fn policy_create_rejects_reflected_operands_before_guard_reads() {
    let (ctx, _) = RequestContext::new();
    for condition in [
        json!({"equals": {"x": "toy%2Fbearer"}}),
        json!({"equals": {"toy/bearer": ":"}}),
    ] {
        let server = Server::start(vec![]);
        let mut input = input();
        input.statements[0].condition = Some(serde_json::from_value(condition).unwrap());
        let error = client(&server, BEARER)
            .create_policy(&ctx, input)
            .await
            .unwrap_err();
        assert!(error.error().has_identity(ErrorClass::InvalidInput));
        assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(server.requests(), 0);
    }
}

#[tokio::test]
async fn policy_create_never_polls_an_unproven_identity() {
    let (ctx, _) = RequestContext::new();
    for (body, partial) in [
        (br#"{"id":"pol-1","ID":"pol-2"}"#.as_slice(), None),
        (br#"{"id":"toy%2Fbearer"}"#, None),
        (br#"{"id":"pol_1"}"#, Some("pol_1")),
    ] {
        let server = Server::start(vec![caller(), catalog(), response("201 Created", body)]);
        let failure = client(&server, BEARER)
            .create_policy(&ctx, input())
            .await
            .unwrap_err();
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert_eq!(
            failure.partial().map(|result| result.policy.id.as_str()),
            partial
        );
        assert!(!failure.error().to_string().contains("toy%2Fbearer"));
        assert_eq!(server.requests(), 3);
    }
}

#[tokio::test]
async fn encoded_credential_never_becomes_a_policy_confirmation_id() {
    let bearer = "a?b";
    let encoded = data_encoding::BASE64URL_NOPAD.encode(bearer.as_bytes());
    let id = format!("x{encoded}tail");
    let body = json!({"id": id}).to_string();
    let server = Server::start(vec![
        caller(),
        catalog(),
        response("201 Created", body.as_bytes()),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, bearer)
        .create_policy(&ctx, input())
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.partial().is_none());
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.operation(), "iam.CreatePolicy");
    assert!(api.message().starts_with("create response had no id"));
    assert!(!api.message().contains(&encoded));
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn policy_confirmation_redacts_values_in_the_sdk_output() {
    let (ctx, _) = RequestContext::new();
    let body = br#"{"id":"pol-1","name":"before toy%2Fbearer after","statements":[{"effect":"allow","resources":["*"],"actions":["iam:ListActions"],"condition":{"equals":{"x":"toy%2Fbearer","toy/bearer":true}}}]}"#;
    let server = Server::start(vec![
        caller(),
        catalog(),
        response("201 Created", br#"{"id":"pol-1"}"#),
        response("200 OK", body),
    ]);
    let output = client(&server, BEARER)
        .create_policy(&ctx, input())
        .await
        .unwrap();
    let projection = serde_json::to_value(output).unwrap();
    assert_eq!(projection["Policy"]["Name"], "<redacted>");
    assert_eq!(
        projection["Policy"]["Statements"][0]["Condition"],
        json!({"equals":{"x":"<redacted>"}})
    );
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn policy_confirmation_redacts_rendered_numbers() {
    let (ctx, _) = RequestContext::new();
    let body = br#"{"id":"pol-1","createdAt":1000000,"statements":[{"effect":"allow","condition":{"equals":{"x":1e6}}}]}"#;
    let server = Server::start(vec![
        caller(),
        catalog(),
        response("201 Created", br#"{"id":"pol-1"}"#),
        response("200 OK", body),
    ]);
    let output = client(&server, "1000000")
        .create_policy(&ctx, input())
        .await
        .unwrap();
    let projection = serde_json::to_value(output).unwrap();
    assert_eq!(projection["Policy"]["CreatedAt"], "<redacted>");
    assert_eq!(
        projection["Policy"]["Statements"][0]["Condition"],
        json!({"equals":{"x":"<redacted>"}})
    );
}

#[tokio::test]
async fn truncated_policy_confirmation_retains_only_the_acknowledged_id() {
    let (ctx, _) = RequestContext::new();
    let bytes = br#"{"id":"pol-1","description":"unconfirmed"}"#;
    let server = Server::start(vec![
        caller(),
        catalog(),
        response("201 Created", br#"{"id":"pol-1"}"#),
        Response::Http {
            status: "200 OK".into(),
            headers: vec![("Content-Length".into(), (bytes.len() + 10).to_string())],
            body: bytes.to_vec(),
        },
    ]);
    let failure = client(&server, BEARER)
        .create_policy(&ctx, input())
        .await
        .unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    let output = serde_json::to_value(failure.partial().unwrap()).unwrap();
    assert_eq!(output["Policy"]["ID"], POLICY_ID);
    assert_eq!(output["Policy"]["Description"], "");
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn policy_delete_refuses_an_attachment_with_not_sent_evidence() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![
        caller(),
        response("200 OK", br#"{"id":"pol-1","manager":"user"}"#),
        response("200 OK", b"[]"),
        response("200 OK", b"[]"),
        response("200 OK", br#"["sa-1"]"#),
    ]);
    let failure = client(&server, BEARER)
        .delete_policy(&ctx, DeletePolicyInput::new(POLICY_ID))
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().detail::<WriteError>(),
        Some(&WriteError::ResourceInUse)
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 5);
    assert!(server.bodies().iter().all(Vec::is_empty));
}

#[tokio::test]
async fn policy_condition_operators_require_objects_or_null() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![]);
    let mut input = input();
    input.statements[0].condition = Some([("equals".into(), Value::Bool(false))].into());
    let failure = client(&server, BEARER)
        .create_policy(&ctx, input)
        .await
        .unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::InvalidInput));
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn replacement_refuses_statements_it_cannot_preserve() {
    for field in ["Condition", "extraRestriction"] {
        let policy = format!(
            r#"{{"id":"pol-1","name":"reader","description":"old","manager":"user","statements":[{{"effect":"allow","actions":["iam:ListActions"],"resources":["*"],"{field}":{{"equals":{{"x":"only"}}}}}}]}}"#
        );
        let server = Server::start(vec![
            caller(),
            response("200 OK", policy.as_bytes()),
            catalog(),
            response("200 OK", b"[]"),
            response("200 OK", b"[]"),
            response("200 OK", b"[]"),
            response("204 No Content", b""),
            response("200 OK", policy.as_bytes()),
        ]);
        let (ctx, _) = RequestContext::new();
        let mut input = UpdatePolicyInput::new(POLICY_ID);
        input.description = Some("new".into());
        let failure = client(&server, BEARER)
            .update_policy(&ctx, input)
            .await
            .unwrap_err();
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(server.requests(), 2);
    }
}

#[tokio::test]
async fn replacement_preserves_canonical_condition_numbers_and_nulls() {
    let policy = br#"{"id":"pol-1","name":"reader","description":"old","manager":"user","statements":[{"effect":"allow","actions":["iam:ListActions"],"resources":["*"],"condition":{"equals":{"x":9007199254740993,"y":null}}}]}"#;
    let server = Server::start(vec![
        caller(),
        response("200 OK", policy),
        catalog(),
        response("200 OK", b"[]"),
        response("200 OK", b"[]"),
        response("200 OK", b"[]"),
        response("204 No Content", b""),
        response("200 OK", policy),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = UpdatePolicyInput::new(POLICY_ID);
    input.description = Some("new".into());
    client(&server, BEARER)
        .update_policy(&ctx, input)
        .await
        .unwrap();
    let request: Value = serde_json::from_slice(&server.bodies()[6]).unwrap();
    let expected: Value = serde_json::from_slice(policy).unwrap();
    assert!(request["statements"] == expected["statements"]);
    assert!(request["description"] == "new");
}

#[tokio::test]
async fn attachment_guard_refuses_noncanonical_pagination_metadata() {
    let server = Server::start(vec![
        caller(),
        catalog(),
        response("200 OK", br#"{"data":[],"totalItems":0,"TotalPages":2}"#),
        response("200 OK", b"[]"),
        response(
            "200 OK",
            br#"{"id":"pol-1","statements":[{"effect":"allow","actions":["iam:ListActions"]}]}"#,
        ),
        response("204 No Content", b""),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, BEARER)
        .attach_user_policy(&ctx, AttachUserPolicyInput::new(POLICY_ID, "usr-2"))
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
#[ignore = "requires hexadecimal credential screening in the shared matcher"]
async fn hex_credential_never_becomes_a_policy_confirmation_id() {
    let encoded = data_encoding::HEXLOWER.encode(BEARER.as_bytes());
    let body = json!({"id": format!("pol-{encoded}")}).to_string();
    let server = Server::start(vec![
        caller(),
        catalog(),
        response("201 Created", body.as_bytes()),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, BEARER)
        .create_policy(&ctx, input())
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.partial().is_none());
    assert!(!failure.error().to_string().contains(&encoded));
    assert_eq!(server.requests(), 3);
}
