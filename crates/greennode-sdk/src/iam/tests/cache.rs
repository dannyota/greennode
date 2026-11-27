use super::{client, response};
use crate::iam::Api;
use crate::iam::policy_guard::Guard;
use crate::iam::writes::write_operation;
use crate::iam::{Client, CreatePolicyInput, CreateUserInput, Statement};
use crate::wire::product::evidence::Evidence;
use crate::wire::product::redirect::Retry;
use crate::wire::test_server::Server;
use crate::{
    Config, ConfigOptions, CredentialsProvider, EndpointOverrides, Error, ErrorClass,
    RecoveryAdvice, RequestContext, Secret, Token, WriteDispatch,
};
use http::{Method, StatusCode};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const CALLER: &[u8] =
    br#"{"userId":"usr-1","userType":"iam-user","username":"alice","accountId":7}"#;
const CATALOG: &[u8] = br#"[{"action":"CreatePolicy","label":"Write","resources":[]}]"#;

#[tokio::test]
async fn caller_is_read_once_per_client_and_shared_by_clones() {
    let server = Server::start(vec![response("200 OK", CALLER)]);
    let client = client(&server, "toy-bearer");
    let (ctx, _) = RequestContext::new();
    for c in [&client, &client.clone(), &client] {
        let caller = c
            .require_classified_caller::<()>(&ctx, "iam.DeleteGroup", &mut Evidence::new())
            .await
            .unwrap();
        assert_eq!(caller.user_id, "usr-1");
    }
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn cached_caller_reflecting_a_new_credential_is_refused() {
    let server = Server::start(vec![response("200 OK", CALLER)]);
    let client = client(&server, "toy-bearer");
    let (ctx, _) = RequestContext::new();
    client
        .require_classified_caller::<()>(&ctx, "iam.DeleteGroup", &mut Evidence::new())
        .await
        .unwrap();
    let mut evidence = Evidence::new();
    evidence.add_secret(&Secret::new("usr-1"));
    let refused = client
        .require_classified_caller::<()>(&ctx, "iam.DeleteGroup", &mut evidence)
        .await;
    let text = refused
        .unwrap_err()
        .error()
        .detail::<crate::RecoveryAdvice>()
        .unwrap()
        .message()
        .to_owned();
    assert!(
        text.contains("iam: iam.GetCallerIdentity: unsafe cached guard response"),
        "{text}"
    );
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn write_actions_are_read_once_and_rechecked() {
    let server = Server::start(vec![response("200 OK", CATALOG)]);
    let client = client(&server, "toy-bearer");
    let (ctx, _) = RequestContext::new();
    let first = client
        .guard_write_actions::<()>(&ctx, true, &mut Evidence::new())
        .await
        .unwrap();
    let second = client
        .guard_write_actions::<()>(&ctx, true, &mut Evidence::new())
        .await
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(server.requests(), 1);
    let mut evidence = Evidence::new();
    evidence.add_secret(&Secret::new("iam:createpolicy"));
    let refused = client
        .guard_write_actions::<()>(&ctx, true, &mut evidence)
        .await
        .unwrap_err();
    let text = refused
        .error()
        .detail::<crate::RecoveryAdvice>()
        .unwrap()
        .message()
        .to_owned();
    assert!(
        text.contains("iam: iam.ListActions: unsafe cached guard response"),
        "{text}"
    );
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn policy_guard_shares_caller_and_actions_across_operations() {
    let server = Server::start(vec![
        response("200 OK", CALLER),
        response("200 OK", CATALOG),
    ]);
    let client = client(&server, "toy-bearer");
    let (ctx, _) = RequestContext::new();
    for _ in 0..2 {
        let mut evidence = Evidence::new();
        let mut guard = Guard::new::<()>(&client, &ctx, "iam.CreatePolicy", &mut evidence)
            .await
            .unwrap();
        guard.load_actions::<()>().await.unwrap();
    }
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn concurrent_first_uses_send_one_caller_and_one_actions_read() {
    let server = Server::start(vec![
        response("200 OK", CALLER),
        response("200 OK", CATALOG),
    ]);
    let client = client(&server, "toy-bearer");
    let (ctx, _) = RequestContext::new();
    let (mut e1, mut e2, mut e3, mut e4) = (
        Evidence::new(),
        Evidence::new(),
        Evidence::new(),
        Evidence::new(),
    );
    let (a, b) = tokio::join!(
        client.require_classified_caller::<()>(&ctx, "iam.DeleteGroup", &mut e1),
        client.require_classified_caller::<()>(&ctx, "iam.DeleteGroup", &mut e2),
    );
    let (c, d) = tokio::join!(
        client.guard_write_actions::<()>(&ctx, true, &mut e3),
        client.guard_write_actions::<()>(&ctx, true, &mut e4),
    );
    assert!(a.is_ok() && b.is_ok() && c.is_ok() && d.is_ok());
    assert_eq!(server.requests(), 2);
}

struct Tokens {
    next: AtomicUsize,
    tokens: Vec<&'static str>,
}

impl CredentialsProvider for Tokens {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        let at = self.next.fetch_add(1, Ordering::Relaxed);
        Box::pin(async move {
            self.tokens
                .get(at)
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

fn provider_client(server: &Server, tokens: Vec<&'static str>) -> Client {
    let config = Config::new(
        ConfigOptions::default()
            .with_region("han-1".into())
            .with_credentials_provider(Arc::new(Tokens {
                next: AtomicUsize::new(0),
                tokens,
            }))
            .with_endpoint_overrides(EndpointOverrides {
                dashboard: Some(format!("{}/dashboard/", server.origin)),
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

fn write_op() -> crate::wire::product::operation::Operation {
    write_operation(
        "iam.DeleteGroup",
        Method::POST,
        "policies-api/v1/groups/x",
        Retry::Repeatable,
        &[StatusCode::OK],
    )
}

fn message<T>(failure: &crate::Failure<T>) -> String {
    failure
        .error()
        .detail::<RecoveryAdvice>()
        .map(|advice| advice.message().to_owned())
        .unwrap_or_default()
}

#[tokio::test]
async fn write_bearer_matching_a_cached_value_is_refused_before_the_write_is_sent() {
    let server = Server::start(vec![response("200 OK", CALLER), response("200 OK", b"{}")]);
    let client = provider_client(&server, vec!["toy-first", "usr-1"]);
    let (ctx, _) = RequestContext::new();
    client
        .require_classified_caller::<()>(&ctx, "iam.DeleteGroup", &mut Evidence::new())
        .await
        .unwrap();
    let mut evidence = Evidence::new();
    client
        .require_classified_caller::<()>(&ctx, "iam.DeleteGroup", &mut evidence)
        .await
        .unwrap();
    let refused = client
        .write::<bool, ()>(&ctx, Api::Policies, &write_op(), &[], None, &mut evidence)
        .await
        .unwrap_err();
    assert_eq!(
        message(&refused),
        "iam: iam.GetCallerIdentity: unsafe cached guard response"
    );
    assert_eq!(refused.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn write_that_used_no_cached_value_is_not_rechecked() {
    let server = Server::start(vec![
        response("200 OK", CALLER),
        response("200 OK", b"true"),
    ]);
    let client = client(&server, "toy-bearer");
    let (ctx, _) = RequestContext::new();
    client
        .require_classified_caller::<()>(&ctx, "iam.DeleteGroup", &mut Evidence::new())
        .await
        .unwrap();
    let mut evidence = Evidence::new();
    evidence.add_secret(&Secret::new("usr-1"));
    client
        .write::<bool, ()>(&ctx, Api::Policies, &write_op(), &[], None, &mut evidence)
        .await
        .unwrap();
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn create_user_on_a_warm_client_ignores_the_cache() {
    let user =
        String::from_utf8(include_bytes!("../../../tests/fixtures/iam/get_user.json").to_vec())
            .unwrap()
            .replace("vngcloud-probe-0000aaaa", "worker");
    let server = Server::start(vec![
        response(
            "200 OK",
            br#"{"userId":"usr-1","userType":"iam-user","username":"xtoy-pass-123x","accountId":7}"#,
        ),
        response(
            "201 Created",
            br#"{"id":"00000000-0000-4000-8000-000000000001"}"#,
        ),
        response("200 OK", user.as_bytes()),
    ]);
    let client = client(&server, "toy-bearer");
    let (ctx, _) = RequestContext::new();
    client
        .require_classified_caller::<()>(&ctx, "iam.DeleteGroup", &mut Evidence::new())
        .await
        .unwrap();
    let mut input = CreateUserInput::new("worker");
    input.password = Some(Secret::new("toy-pass-123"));
    let created = client.create_user(&ctx, input).await.unwrap();
    assert_eq!(created.user.username, "worker");
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn policy_write_keeps_the_api_error_when_the_action_read_fails() {
    let server = Server::start(vec![
        response("200 OK", CALLER),
        response("403 Forbidden", br#"{"message":"no access"}"#),
    ]);
    let client = client(&server, "toy-bearer");
    let (ctx, _) = RequestContext::new();
    let mut statement = Statement::default();
    statement.effect = "allow".into();
    statement.actions = vec!["iam:ListActions".into()];
    statement.resources = vec!["*".into()];
    let failure = client
        .create_policy(&ctx, CreatePolicyInput::new("reader", vec![statement]))
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.status(), Some(403));
    assert_eq!(api.message(), "no access");
}

#[tokio::test]
async fn lock_wait_is_bounded_by_the_request_context() {
    let server = Server::start(vec![]);
    let client = client(&server, "toy-bearer");
    let (ctx, cancel) = RequestContext::new();
    let held = client.guard.caller.lock().await;
    let mut evidence = Evidence::new();
    let wait = client.require_classified_caller::<()>(&ctx, "iam.DeleteGroup", &mut evidence);
    let cancel_after_first_poll = async {
        tokio::task::yield_now().await;
        cancel.cancel();
    };
    let (waited, ()) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(5), wait),
        cancel_after_first_poll
    );
    let waited = waited.expect("lock wait ignored the context");
    drop(held);
    assert_eq!(waited.unwrap_err().write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn write_bearer_matching_a_cached_action_name_is_refused_before_the_write_is_sent() {
    let server = Server::start(vec![response("200 OK", CATALOG), response("200 OK", b"{}")]);
    let client = provider_client(&server, vec!["toy-first", "iam:createpolicy"]);
    let (ctx, _) = RequestContext::new();
    client
        .guard_write_actions::<()>(&ctx, true, &mut Evidence::new())
        .await
        .unwrap();
    let mut evidence = Evidence::new();
    client
        .guard_write_actions::<()>(&ctx, true, &mut evidence)
        .await
        .unwrap();
    let refused = client
        .write::<bool, ()>(&ctx, Api::Policies, &write_op(), &[], None, &mut evidence)
        .await
        .unwrap_err();
    assert_eq!(
        message(&refused),
        "iam: iam.ListActions: unsafe cached guard response"
    );
    assert_eq!(refused.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn bearer_refreshed_by_401_recovery_is_compared_before_the_resend() {
    let server = Server::start(vec![
        response("200 OK", CALLER),
        response("401 Unauthorized", b"{}"),
        response("200 OK", b"true"),
    ]);
    let client = provider_client(&server, vec!["toy-first", "toy-second", "usr-1"]);
    let (ctx, _) = RequestContext::new();
    client
        .require_classified_caller::<()>(&ctx, "iam.DeleteGroup", &mut Evidence::new())
        .await
        .unwrap();
    let mut evidence = Evidence::new();
    client
        .require_classified_caller::<()>(&ctx, "iam.DeleteGroup", &mut evidence)
        .await
        .unwrap();
    let refused = client
        .write::<bool, ()>(&ctx, Api::Policies, &write_op(), &[], None, &mut evidence)
        .await
        .unwrap_err();
    assert_eq!(
        message(&refused),
        "iam: iam.GetCallerIdentity: unsafe cached guard response"
    );
    assert!(
        refused
            .error()
            .api_error()
            .and_then(|api| api.status())
            .is_none()
    );
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn fresh_action_names_that_reflect_use_the_cached_response_text() {
    let server = Server::start(vec![response("200 OK", CATALOG)]);
    let client = client(&server, "toy-bearer");
    let (ctx, _) = RequestContext::new();
    let mut evidence = Evidence::new();
    evidence.add_secret(&Secret::new("iam:createpolicy"));
    let refused = client
        .guard_write_actions::<()>(&ctx, true, &mut evidence)
        .await
        .unwrap_err();
    assert_eq!(
        message(&refused),
        "iam: iam.ListActions: unsafe cached guard response"
    );
}
