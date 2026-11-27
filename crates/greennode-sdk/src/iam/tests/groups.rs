use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use super::{client, response};
use crate::iam::{Client, CreateGroupInput, DeleteGroupInput, UpdateGroupInput};
use crate::wire::test_server::Server;
use crate::{
    Config, ConfigOptions, CredentialsProvider, EndpointOverrides, Error, ErrorClass,
    RecoveryAdvice, RequestContext, Secret, Token, WriteDispatch,
};
use serde_json::{Value, json};

#[tokio::test]
async fn create_and_update_confirm_on_the_iam_endpoint() {
    let existing = br#"{"id":"group-1","name":"team","description":"old"}"#;
    let updated = br#"{"id":"group-1","name":"team","description":"new"}"#;
    let server = Server::start(vec![
        response("201 Created", br#"{"id":"group-1"}"#),
        response("200 OK", existing),
        response("200 OK", existing),
        response("502 Bad Gateway", b"{}"),
        response("204 No Content", b""),
        response("200 OK", updated),
    ]);
    let client = client(&server, "toy-bearer");
    let (ctx, _) = RequestContext::new();
    let created = client
        .create_group(&ctx, CreateGroupInput::new("team"))
        .await
        .unwrap();
    assert_eq!(created.group.id, "group-1");
    let mut input = UpdateGroupInput::new("group-1");
    input.description = Some("new".into());
    let changed = client.update_group(&ctx, input).await.unwrap();
    assert_eq!(changed.group.description, "new");
    let heads = server.heads();
    for (head, route) in heads.iter().zip([
        "POST /iam/policies-api/v1/groups ",
        "GET /iam/policies-api/v1/groups/group-1 ",
        "GET /iam/policies-api/v1/groups/group-1 ",
        "PATCH /iam/policies-api/v1/groups/group-1 ",
        "PATCH /iam/policies-api/v1/groups/group-1 ",
        "GET /iam/policies-api/v1/groups/group-1 ",
    ]) {
        assert!(head.starts_with(route.as_bytes()));
    }
    assert_eq!(heads.len(), 6);
    let bodies = server.bodies();
    assert!(
        serde_json::from_slice::<Value>(&bodies[0]).ok()
            == Some(json!({
                "name":"team", "description":"", "mode":"iam"
            }))
    );
    assert!(
        serde_json::from_slice::<Value>(&bodies[3]).ok()
            == Some(json!({
                "name":"team", "description":"new"
            }))
    );
    assert!(bodies[3] == bodies[4]);
}

#[tokio::test]
async fn create_never_replays_or_confirms_an_ambiguous_identity() {
    let (ctx, _) = RequestContext::new();
    for (status, body) in [
        ("502 Bad Gateway", b"{}".as_slice()),
        ("201 Created", br#"{"id":"group-1","ID":"group-1"}"#),
        ("201 Created", br#"{"id":"toy-bearer"}"#),
    ] {
        let server = Server::start(vec![response(status, body)]);
        let err = client(&server, "toy-bearer")
            .create_group(&ctx, CreateGroupInput::new("team"))
            .await
            .unwrap_err();
        assert_eq!(err.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        let (error, partial, _) = err.into_parts();
        assert!(partial.is_none());
        assert!(!error.to_string().contains("toy-bearer"));
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn group_update_refuses_reflecting_preflight_and_preserves_safe_partial() {
    let (ctx, _) = RequestContext::new();
    let reflected = Server::start(vec![response(
        "200 OK",
        br#"{"id":"group-1","name":"team","description":"toy%2Fbearer"}"#,
    )]);
    let refused = client(&reflected, "toy/bearer")
        .update_group(&ctx, UpdateGroupInput::new("group-1"))
        .await
        .unwrap_err();
    assert_eq!(refused.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(reflected.requests(), 1);
    let server = Server::start(vec![
        response(
            "200 OK",
            br#"{"id":"group-1","name":"team","description":"ordinary"}"#,
        ),
        response("204 No Content", b""),
        response("404 Not Found", b"{}"),
    ]);
    let client = client(&server, "toy/bearer");
    let failed = client
        .update_group(&ctx, UpdateGroupInput::new("group-1"))
        .await
        .unwrap_err();
    let (error, partial, dispatch) = failed.into_parts();
    assert!(error.has_identity(ErrorClass::NotSettled));
    assert_eq!(dispatch, WriteDispatch::MayHaveBeenSent);
    let output = serde_json::to_value(partial.unwrap()).unwrap();
    assert_eq!(output["Group"]["ID"], "group-1");
    assert_eq!(output["Group"]["Description"], "ordinary");
    let mut rejected = UpdateGroupInput::new("group-1");
    rejected.description = Some("toy%2Fbearer".into());
    let err = client.update_group(&ctx, rejected).await.unwrap_err();
    assert_eq!(err.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn delete_requires_proven_empty_iam_membership() {
    let (ctx, _) = RequestContext::new();
    let caller = br#"{"userId":"user-1","userType":"iam-user"}"#;
    for body in [
        br#"{"id":"group-1","mode":"iam","iamUsers":[],"policies":["policy-1"]}"#.as_slice(),
        br#"{"id":"group-1","mode":"iam","iamUsers":[],"policies":null}"#,
        br#"{"id":"group-1","mode":"idp","iamUsers":[],"policies":[]}"#,
        br#"{"id":"group-2","mode":"iam","iamUsers":[],"policies":[]}"#,
        br#"{"id":"group-1","mode":"iam","iamUsers":[],"policies":[],"unknown":true}"#,
    ] {
        let server = Server::start(vec![response("200 OK", caller), response("200 OK", body)]);
        let failed = client(&server, "toy-bearer")
            .delete_group(&ctx, DeleteGroupInput::new("group-1"))
            .await
            .unwrap_err();
        assert_eq!(failed.write_dispatch(), WriteDispatch::NotSent);
        let heads = server.heads();
        assert_eq!(heads.len(), 2);
        assert!(heads[0].starts_with(b"GET /dashboard/accounts-api/v1/auth/userinfo "));
        assert!(heads[1].starts_with(b"GET /iam/policies-api/v1/groups/group-1 "));
    }
    let server = Server::start(vec![
        response("200 OK", caller),
        response(
            "200 OK",
            br#"{"id":"group-1","mode":"iam","iamUsers":[],"policies":[]}"#,
        ),
        response("204 No Content", b""),
    ]);
    client(&server, "toy-bearer")
        .delete_group(&ctx, DeleteGroupInput::new("group-1"))
        .await
        .unwrap();
    assert!(server.heads()[2].starts_with(b"DELETE /iam/policies-api/v1/groups/group-1 "));
}

#[tokio::test]
async fn path_invalid_group_id_is_retained_without_a_confirmation_request() {
    let server = Server::start(vec![response("201 Created", br#"{"id":"group_1"}"#)]);
    let (ctx, _) = RequestContext::new();
    let err = client(&server, "toy-bearer")
        .create_group(&ctx, CreateGroupInput::new("team"))
        .await
        .unwrap_err();
    let (error, partial, dispatch) = err.into_parts();
    assert_eq!(dispatch, WriteDispatch::MayHaveBeenSent);
    assert!(error.has_identity(ErrorClass::NotSettled));
    let mut group = crate::iam::Group::default();
    group.id = "group_1".to_owned();
    assert_eq!(partial.unwrap().group, group);
    assert_eq!(server.requests(), 1);
}

struct ConfirmationProvider {
    attempts: AtomicUsize,
    tokens: Vec<&'static str>,
}

fn confirmation_client(server: &Server, provider: Arc<ConfirmationProvider>) -> Client {
    let config = Config::new(
        ConfigOptions::default()
            .with_region("han-1".into())
            .with_credentials_provider(provider)
            .with_endpoint_overrides(EndpointOverrides {
                iam: Some(format!("{}/iam/", server.origin)),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    Client::new(&config)
}

impl CredentialsProvider for ConfirmationProvider {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        let attempt = self.attempts.fetch_add(1, Ordering::Relaxed);
        Box::pin(async move {
            self.tokens
                .get(attempt)
                .map(|token| Token::new(Secret::new(*token), None))
                .ok_or_else(|| Error::new(ErrorClass::Auth))
        })
    }

    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn failed_confirmation_sign_in_withholds_id_from_recovery_advice() {
    let server = Server::start(vec![
        response("201 Created", br#"{"id":"group-1"}"#),
        response("401 Unauthorized", b"{}"),
    ]);
    let provider = Arc::new(ConfirmationProvider {
        attempts: AtomicUsize::new(0),
        tokens: vec!["toy-bearer", "toy-bearer"],
    });
    let client = confirmation_client(&server, provider.clone());
    let (ctx, _) = RequestContext::new();
    let err = client
        .create_group(&ctx, CreateGroupInput::new("team"))
        .await
        .unwrap_err();
    let (error, partial, dispatch) = err.into_parts();
    assert_eq!(dispatch, WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 2);
    assert_eq!(provider.attempts.load(Ordering::Relaxed), 3);
    assert!(error.has_identity(ErrorClass::NotSettled));
    assert!(partial.is_none());
    assert_eq!(
        error.detail::<RecoveryAdvice>().unwrap().message(),
        "vngcloud: write outcome not confirmed"
    );
}

#[tokio::test]
async fn short_base64_credential_in_created_group_id_is_withheld() {
    let bearer = "a?bcd";
    let encoded = data_encoding::BASE64URL_NOPAD.encode(bearer.as_bytes());
    let body = json!({"id": encoded}).to_string();
    let server = Server::start(vec![response("201 Created", body.as_bytes())]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, bearer)
        .create_group(&ctx, CreateGroupInput::new("team"))
        .await
        .err()
        .unwrap();
    let (error, partial, dispatch) = failure.into_parts();
    assert_eq!(dispatch, WriteDispatch::MayHaveBeenSent);
    assert!(partial.is_none());
    let message = error.api_error().unwrap().message();
    assert!(!message.contains(&encoded));
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn prefixed_short_base64_credentials_never_become_group_ids() {
    for (bearer, encoding) in [
        ("a?bcd", data_encoding::BASE64_NOPAD),
        ("ab>de", data_encoding::BASE64URL_NOPAD),
    ] {
        let id = format!("group-{}", encoding.encode(bearer.as_bytes()));
        let body = json!({"id": id}).to_string();
        let server = Server::start(vec![
            response("201 Created", body.as_bytes()),
            response("404 Not Found", b"{}"),
        ]);
        let (ctx, _) = RequestContext::new();
        let failure = client(&server, bearer)
            .create_group(&ctx, CreateGroupInput::new("team"))
            .await
            .err()
            .unwrap();
        let (error, partial, dispatch) = failure.into_parts();
        assert_eq!(dispatch, WriteDispatch::MayHaveBeenSent);
        assert_eq!(server.requests(), 1);
        assert!(partial.is_none());
        assert!(!error.api_error().unwrap().message().contains(&id));
    }
}

#[tokio::test]
async fn a_new_bearer_before_confirmation_blocks_the_group_id_and_partial() {
    let bearer = "ab>de";
    let id = format!(
        "group-{}",
        data_encoding::BASE64URL_NOPAD.encode(bearer.as_bytes())
    );
    check_new_bearer_during_confirmation(&id, bearer).await;
}

#[tokio::test]
async fn a_bearer_equal_to_the_group_id_never_reaches_recovery_output() {
    check_new_bearer_during_confirmation("group-1", "group-1").await;
}

async fn check_new_bearer_during_confirmation(id: &str, bearer: &'static str) {
    let body = json!({"id": id}).to_string();
    let server = Server::start(vec![
        response("201 Created", body.as_bytes()),
        response("404 Not Found", b"{}"),
    ]);
    let client = confirmation_client(
        &server,
        Arc::new(ConfirmationProvider {
            attempts: AtomicUsize::new(0),
            tokens: ["toy-bearer", bearer].into(),
        }),
    );
    let (ctx, _) = RequestContext::new();
    let failure = client
        .create_group(&ctx, CreateGroupInput::new("team"))
        .await
        .err()
        .unwrap();
    let (error, partial, dispatch) = failure.into_parts();
    assert_eq!(dispatch, WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 1);
    assert!(partial.is_none());
    assert!(
        !error
            .detail::<RecoveryAdvice>()
            .unwrap()
            .message()
            .contains(id)
    );
}

#[tokio::test]
async fn scoped_group_ids_are_screened_in_updates_and_partials() {
    let bearer = "ab>de";
    let encoded = data_encoding::BASE64URL_NOPAD.encode(bearer.as_bytes());
    let body = json!({
        "id": "group-1",
        "name": "team",
        "iamUsers": [format!("user-{encoded}"), "user-1".to_owned()],
        "policies": [format!("policy-{encoded}"), "policy-1".to_owned()],
    })
    .to_string();
    let (ctx, _) = RequestContext::new();
    for confirmed in [true, false] {
        let server = Server::start(vec![
            response("200 OK", body.as_bytes()),
            response("204 No Content", b""),
            if confirmed {
                response("200 OK", body.as_bytes())
            } else {
                response("404 Not Found", b"{}")
            },
        ]);
        let client = confirmation_client(
            &server,
            Arc::new(ConfirmationProvider {
                attempts: AtomicUsize::new(0),
                tokens: vec!["toy-bearer", "toy-bearer", bearer],
            }),
        );
        let result = client
            .update_group(&ctx, UpdateGroupInput::new("group-1"))
            .await;
        let group = if confirmed {
            result.unwrap().group
        } else {
            let (error, partial, dispatch) = result.unwrap_err().into_parts();
            assert!(error.has_identity(ErrorClass::NotSettled));
            assert_eq!(dispatch, WriteDispatch::MayHaveBeenSent);
            partial.unwrap().group
        };
        let output = serde_json::to_value(group).unwrap();
        assert!(output["UserIDs"] == json!(["<redacted>", "user-1"]));
        assert!(output["PolicyIDs"] == json!(["<redacted>", "policy-1"]));
        assert_eq!(server.requests(), 3);
    }
}
