use super::{client, client_with_options, response};
use crate::iam::{
    AttachServiceAccountPolicyInput, AttachUserPolicyInput, CreatePolicyInput, DeletePolicyInput,
    DetachServiceAccountPolicyInput, Statement, UpdatePolicyInput, WriteError,
};
use crate::wire::test_server::{Response, Server};
use crate::{ConfigOptions, ErrorClass, RequestContext, WriteDispatch};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct RefreshingProvider(AtomicUsize);

impl crate::CredentialsProvider for RefreshingProvider {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>,
    > {
        let value = if self.0.load(Ordering::SeqCst) == 0 {
            "first-token"
        } else {
            "second-token"
        };
        Box::pin(async move { Ok(crate::Token::new(crate::Secret::new(value), None)) })
    }

    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a crate::Secret,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), crate::Error>> + Send + 'a>>
    {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn account_policy_retries_401_after_provider_refresh() {
    for detach in [false, true] {
        let server = Server::start(vec![caller(), catalog(), response("200 OK", br#"{"data":[],"totalItems":0}"#), response("200 OK", br#"{"id":"pol-1","statements":[{"effect":"allow","actions":["iam:ListActions"]}]}"#), response("401 Unauthorized", b"{}"), response("403 Forbidden", b"{}")]);
        let provider = Arc::new(RefreshingProvider(AtomicUsize::new(0)));
        let config = crate::Config::new(
            crate::ConfigOptions::default()
                .with_region("han-1".into())
                .with_project_id("unscoped".into())
                .with_credentials_provider(provider.clone())
                .with_endpoint_overrides(crate::EndpointOverrides {
                    dashboard: Some(format!("{}/dashboard/", server.origin)),
                    iam: Some(format!("{}/iam/", server.origin)),
                    ..Default::default()
                }),
        )
        .unwrap();
        config
            .session()
            .set_test_product_root(server.root_der.clone());
        let client = crate::iam::Client::new(&config);
        let (ctx, _) = RequestContext::new();
        let (error, dispatch) = if detach {
            let failure = client
                .detach_service_account_policy(
                    &ctx,
                    DetachServiceAccountPolicyInput::new("pol-1", "sa-1"),
                )
                .await
                .unwrap_err();
            let dispatch = failure.write_dispatch();
            (failure.into_parts().0, dispatch)
        } else {
            let failure = client
                .attach_service_account_policy(
                    &ctx,
                    AttachServiceAccountPolicyInput::new("pol-1", "sa-1"),
                )
                .await
                .unwrap_err();
            let dispatch = failure.write_dispatch();
            (failure.into_parts().0, dispatch)
        };
        assert_eq!(provider.0.load(Ordering::SeqCst), 1);
        assert_eq!(server.requests(), 6);
        let heads = server.heads();
        assert!(
            heads[4]
                .windows(b"first-token".len())
                .any(|part| part == b"first-token")
        );
        assert!(
            heads[5]
                .windows(b"second-token".len())
                .any(|part| part == b"second-token")
        );
        assert_eq!(error.class(), ErrorClass::PermissionDenied);
        let api = error.api_error().unwrap();
        assert_eq!(api.code(), "Forbidden");
        assert_eq!(api.status(), Some(403));
        assert_eq!(dispatch, WriteDispatch::MayHaveBeenSent);
    }
}

#[tokio::test]
async fn account_policy_refuses_service_account_caller_before_policy_reads() {
    let server = Server::start(vec![response(
        "200 OK",
        br#"{"userId":"usr-1","userType":"user-sa"}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, BEARER)
        .attach_service_account_policy(&ctx, AttachServiceAccountPolicyInput::new("pol-1", "sa-1"))
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().detail::<WriteError>(),
        Some(&WriteError::SelfChange)
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn account_policy_response_failures_match_transport_contract() {
    for detach in [false, true] {
        for (kind, expected_class, code, status) in [
            ("truncated", ErrorClass::Api, "", None),
            (
                "oversized",
                ErrorClass::PermissionDenied,
                "Forbidden",
                Some(403),
            ),
            ("redirect", ErrorClass::Api, "", None),
        ] {
            let last = match kind {
                "truncated" => Response::RawClose(
                    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 20\r\n\r\n{}".to_vec(),
                ),
                "oversized" => response("403 Forbidden", &vec![b'x'; 8 * 1024 * 1024 + 1]),
                _ => Response::Http {
                    status: "307 Temporary Redirect".into(),
                    headers: vec![
                        ("Content-Length".into(), "0".into()),
                        ("Location".into(), "/other".into()),
                    ],
                    body: vec![],
                },
            };
            let server = Server::start(vec![caller(), catalog(), response("200 OK", br#"{"data":[],"totalItems":0}"#), response("200 OK", br#"{"id":"pol-1","statements":[{"effect":"allow","actions":["iam:ListActions"]}]}"#), last]);
            let (ctx, _) = RequestContext::new();
            let client = client(&server, BEARER);
            let (error, dispatch) = if detach {
                let failure = client
                    .detach_service_account_policy(
                        &ctx,
                        DetachServiceAccountPolicyInput::new("pol-1", "sa-1"),
                    )
                    .await
                    .unwrap_err();
                let dispatch = failure.write_dispatch();
                (failure.into_parts().0, dispatch)
            } else {
                let failure = client
                    .attach_service_account_policy(
                        &ctx,
                        AttachServiceAccountPolicyInput::new("pol-1", "sa-1"),
                    )
                    .await
                    .unwrap_err();
                let dispatch = failure.write_dispatch();
                (failure.into_parts().0, dispatch)
            };
            assert_eq!(error.class(), expected_class, "{kind} detach={detach}");
            let api = error.api_error().unwrap();
            assert_eq!(api.code(), code, "{kind} detach={detach}");
            assert_eq!(api.status(), status, "{kind} detach={detach}");
            assert_eq!(dispatch, WriteDispatch::MayHaveBeenSent);
            assert_eq!(server.heads().len(), 5);
        }
    }
}

#[tokio::test]
async fn account_policy_retries_follow_write_method() {
    for detach in [false, true] {
        for (status, class, code, expected_requests) in [
            ("400 Bad Request", ErrorClass::Api, "BadRequest", 5),
            ("404 Not Found", ErrorClass::NotFound, "NotFound", 5),
            ("409 Conflict", ErrorClass::Api, "Conflict", 5),
            (
                "500 Internal Server Error",
                ErrorClass::Api,
                "ServerError",
                5,
            ),
            (
                "429 Too Many Requests",
                ErrorClass::RateLimited,
                "Throttled",
                8,
            ),
            (
                "502 Bad Gateway",
                ErrorClass::Api,
                "ServerError",
                if detach { 8 } else { 5 },
            ),
            ("401 Unauthorized", ErrorClass::Auth, "Unauthorized", 5),
        ] {
            let mut responses = vec![caller(), catalog(), response("200 OK", br#"{"data":[],"totalItems":0}"#), response("200 OK", br#"{"id":"pol-1","statements":[{"effect":"allow","actions":["iam:ListActions"]}]}"#)];
            responses.extend((0..4).map(|_| response(status, b"{}")));
            let server = Server::start(responses);
            let (ctx, _) = RequestContext::new();
            let options = ConfigOptions::default().with_clock(crate::testing::retry_clock());
            let client = client_with_options(&server, BEARER, options);
            let (error, dispatch) = if detach {
                let failure = client
                    .detach_service_account_policy(
                        &ctx,
                        DetachServiceAccountPolicyInput::new("pol-1", "sa-1"),
                    )
                    .await
                    .unwrap_err();
                let dispatch = failure.write_dispatch();
                (failure.into_parts().0, dispatch)
            } else {
                let failure = client
                    .attach_service_account_policy(
                        &ctx,
                        AttachServiceAccountPolicyInput::new("pol-1", "sa-1"),
                    )
                    .await
                    .unwrap_err();
                let dispatch = failure.write_dispatch();
                (failure.into_parts().0, dispatch)
            };
            assert_eq!(error.class(), class, "{status} detach={detach}");
            let api = error.api_error().unwrap();
            assert_eq!(api.code(), code, "{status} detach={detach}");
            assert_eq!(api.status(), Some(status[..3].parse().unwrap()));
            assert_eq!(dispatch, WriteDispatch::MayHaveBeenSent);
            assert_eq!(
                server.requests(),
                expected_requests,
                "{status} detach={detach}"
            );
        }
    }
}

#[tokio::test]
async fn loopback_server_stops_accepting_after_the_configured_response() {
    let server = Server::start_stopping_after(vec![caller()], 1);
    let (ctx, _) = RequestContext::new();
    client(&server, BEARER)
        .get_caller_identity(&ctx, crate::iam::GetCallerIdentityInput)
        .await
        .unwrap();
    assert_eq!(server.requests(), 1);
    assert!(std::net::TcpStream::connect(server.address).is_err());
}

#[tokio::test]
async fn account_policy_failed_dial_after_guard_is_not_sent() {
    for detach in [false, true] {
        let server = Server::start_stopping_after(vec![
            caller(), catalog(), response("200 OK", br#"{"data":[],"totalItems":0}"#),
            response("200 OK", br#"{"id":"pol-1","statements":[{"effect":"allow","actions":["iam:ListActions"]}]}"#),
        ], 4);
        let (ctx, _) = RequestContext::new();
        let options = ConfigOptions::default().with_clock(crate::testing::retry_clock());
        let client = client_with_options(&server, BEARER, options);
        let (error, dispatch) = if detach {
            let failure = client
                .detach_service_account_policy(
                    &ctx,
                    DetachServiceAccountPolicyInput::new("pol-1", "sa-1"),
                )
                .await
                .unwrap_err();
            let dispatch = failure.write_dispatch();
            (failure.into_parts().0, dispatch)
        } else {
            let failure = client
                .attach_service_account_policy(
                    &ctx,
                    AttachServiceAccountPolicyInput::new("pol-1", "sa-1"),
                )
                .await
                .unwrap_err();
            let dispatch = failure.write_dispatch();
            (failure.into_parts().0, dispatch)
        };
        assert_eq!(error.class(), ErrorClass::Api);
        let api = error.api_error().unwrap();
        assert_eq!(api.code(), "");
        assert_eq!(api.status(), None);
        assert_eq!(dispatch, WriteDispatch::NotSent);
        assert_eq!(server.requests(), 4);
        assert_eq!(client.session.test_retry_attempts(), 4);
    }
}

#[tokio::test]
async fn account_policy_accepts_classified_caller_without_path_id() {
    for detach in [false, true] {
        let server = Server::start(vec![response("200 OK", br#"{"userId":"user@example.com","userType":"iam-user"}"#), catalog(), response("200 OK", br#"{"data":[],"totalItems":0}"#), response("200 OK", br#"{"id":"pol-1","statements":[{"effect":"allow","actions":["iam:ListActions"]}]}"#), response("204 No Content", b"")]);
        let (ctx, _) = RequestContext::new();
        let client = client(&server, BEARER);
        if detach {
            client
                .detach_service_account_policy(
                    &ctx,
                    DetachServiceAccountPolicyInput::new("pol-1", "sa-1"),
                )
                .await
                .unwrap();
        } else {
            client
                .attach_service_account_policy(
                    &ctx,
                    AttachServiceAccountPolicyInput::new("pol-1", "sa-1"),
                )
                .await
                .unwrap();
        }
        assert_eq!(server.heads().len(), 5);
    }
}

#[tokio::test]
async fn unclassified_caller_is_not_cached() {
    let server = Server::start(vec![
        response("200 OK", b"{}"),
        caller(),
        catalog(),
        response("200 OK", br#"{"data":[],"totalItems":0}"#),
        response(
            "200 OK",
            br#"{"id":"pol-1","statements":[{"effect":"allow","actions":["iam:ListActions"]}]}"#,
        ),
        response("204 No Content", b""),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, BEARER);
    assert!(
        client
            .attach_service_account_policy(
                &ctx,
                AttachServiceAccountPolicyInput::new("pol-1", "sa-1")
            )
            .await
            .is_err()
    );
    client
        .attach_service_account_policy(&ctx, AttachServiceAccountPolicyInput::new("pol-1", "sa-1"))
        .await
        .unwrap();
    assert_eq!(server.heads().len(), 6);
}

#[tokio::test]
async fn account_policy_checks_required_fields_before_id_syntax() {
    for detach in [false, true] {
        let server = Server::start(vec![]);
        let (ctx, _) = RequestContext::new();
        let client = client(&server, BEARER);
        let error = if detach {
            client
                .detach_service_account_policy(
                    &ctx,
                    DetachServiceAccountPolicyInput::new("bad/id", ""),
                )
                .await
                .unwrap_err()
                .into_parts()
                .0
        } else {
            client
                .attach_service_account_policy(
                    &ctx,
                    AttachServiceAccountPolicyInput::new("bad/id", ""),
                )
                .await
                .unwrap_err()
                .into_parts()
                .0
        };
        assert!(
            error
                .api_error()
                .unwrap()
                .message()
                .ends_with("requires ServiceAccountID")
        );
        assert_eq!(server.heads().len(), 0);
    }
}

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
    let hex = data_encoding::HEXUPPER.encode(BEARER.as_bytes());
    for condition in [
        json!({"equals": {"x": "toy%2Fbearer"}}),
        json!({"equals": {"toy/bearer": ":"}}),
        json!({"equals": {"x": hex}}),
        json!({"equals": {hex: ":"}}),
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
    assert!(
        failure
            .error()
            .detail::<crate::wire::product::operation::RefusedGuard>()
            .is_some()
    );
    assert!(!api.message().contains(&encoded));
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn policy_confirmation_refuses_reflected_values() {
    let (ctx, _) = RequestContext::new();
    let body = br#"{"id":"pol-1","name":"before toy%2Fbearer after","statements":[{"effect":"allow","resources":["*"],"actions":["iam:ListActions"],"condition":{"equals":{"x":"toy%2Fbearer","toy/bearer":true}}}]}"#;
    let server = Server::start(vec![
        caller(),
        catalog(),
        response("201 Created", br#"{"id":"pol-1"}"#),
        response("200 OK", body),
    ]);
    let failure = client(&server, BEARER)
        .create_policy(&ctx, input())
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert_eq!(failure.partial().unwrap().policy.id, "pol-1");
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn policy_confirmation_refuses_reflected_number_spelling() {
    let (ctx, _) = RequestContext::new();
    let body = br#"{"id":"pol-1","createdAt":1000000,"statements":[{"effect":"allow","condition":{"equals":{"x":1e6}}}]}"#;
    let server = Server::start(vec![
        caller(),
        catalog(),
        response("201 Created", br#"{"id":"pol-1"}"#),
        response("200 OK", body),
    ]);
    let failure = client(&server, "1000000")
        .create_policy(&ctx, input())
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert_eq!(failure.partial().unwrap().policy.id, "pol-1");
    assert_eq!(server.requests(), 4);
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

mod settlement;
