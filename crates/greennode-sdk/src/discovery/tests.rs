use super::*;
use crate::wire::{
    connection::{HeadLimit, OneSendClient, PooledClient, Trust},
    product::sender::{self, BearerToken},
    test_server::{Response, Server},
};
use crate::{Clock, RequestContext, Secret, SystemClock, Token};
use std::{future::Future, pin::Pin, time::Duration};

struct Source;
impl BearerSource for Source {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<BearerToken, Error>> + Send + 'a>> {
        Box::pin(async {
            Ok(BearerToken {
                token: Token::new(Secret::new("toy-token"), None),
                obtained_at: None,
                signin_credentials: None,
            })
        })
    }
    fn rejected<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
        _: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}
struct RotatingSource(std::sync::Mutex<&'static str>);
impl BearerSource for RotatingSource {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<BearerToken, Error>> + Send + 'a>> {
        let value = *self.0.lock().unwrap();
        Box::pin(async move {
            Ok(BearerToken {
                token: Token::new(Secret::new(value), None),
                obtained_at: None,
                signin_credentials: None,
            })
        })
    }
    fn rejected<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
        _: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}
struct Sleeper;
impl sender::Sleeper for Sleeper {
    fn sleep(&self, _: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
}
fn response(status: &str, body: &[u8]) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.to_vec(),
    }
}
async fn call(
    discovery: &Discovery,
    server: &Server,
    ctx: &RequestContext,
) -> Result<Project, Box<Error>> {
    call_at(
        discovery,
        server,
        ctx,
        &SystemClock,
        SystemClock.monotonic_now() + Duration::from_secs(5),
    )
    .await
}

async fn call_at(
    discovery: &Discovery,
    server: &Server,
    ctx: &RequestContext,
    clock: &dyn Clock,
    deadline: tokio::time::Instant,
) -> Result<Project, Box<Error>> {
    call_at_source(discovery, server, ctx, clock, deadline, &Source).await
}

async fn call_at_source(
    discovery: &Discovery,
    server: &Server,
    ctx: &RequestContext,
    clock: &dyn Clock,
    deadline: tokio::time::Instant,
    source: &dyn BearerSource,
) -> Result<Project, Box<Error>> {
    let clients = sender::Clients {
        pooled: PooledClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap(),
        one_send: OneSendClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap(),
    };
    let base = crate::endpoint::Base::parse(&server.origin).unwrap();
    discovery
        .require(
            Runtime {
                configured_credentials: None,
                client: &clients,
                ctx,
                clock,
                deadline,
                sleeper: &Sleeper,
                jitter: &mut || 0,
            },
            &base,
            source,
            "han-1",
            ProjectRead::Id,
        )
        .await
}

#[tokio::test]
async fn discovery_rejects_malformed_json_errors_and_screens_complete_identifier_fields() {
    let source = RotatingSource(std::sync::Mutex::new("jkl"));
    for status in ["401 Unauthorized", "503 Service Unavailable"] {
        for body in [
            br#"  {"projectId":"ordinary-id""#.as_slice(),
            br#"[{"projectId":"ordinary-id"}"#,
            br#""unterminated"#,
            b"\xef\xbb\xbf {\"projectId\":\"ordinary-id\"",
            br#"{"projectId":"pre6A6B6Cpost"}"#,
        ] {
            let server = Server::start(vec![
                response(status, body),
                response("200 OK", br#"{"projects":[{"projectId":"one"}]}"#),
            ]);
            let result = call_at_source(
                &Discovery::default(),
                &server,
                &RequestContext::new().0,
                &SystemClock,
                SystemClock.monotonic_now() + Duration::from_secs(5),
                &source,
            )
            .await;
            let Err(error) = result else {
                panic!("discovery retried an unsafe error response");
            };
            assert_eq!(
                error.api_error().map(crate::ApiError::code),
                Some("InvalidResponse")
            );
            assert_eq!(server.requests(), 1);
        }
    }
}

#[tokio::test]
async fn discovery_a_nonaccepted_201_uses_the_ordinary_status_diagnostic() {
    let server = Server::start(vec![response("201 Created", b"unaccepted response")]);
    let error = call(&Discovery::default(), &server, &RequestContext::new().0)
        .await
        .unwrap_err();
    let api = error.api_error().unwrap();
    assert_eq!(api.status(), Some(201));
    assert_eq!(api.message(), "Created");
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn discovery_adds_bearer_to_caller_evidence() {
    let server = Server::start(vec![response(
        "200 OK",
        br#"{"projects":[{"projectId":"one"}]}"#,
    )]);
    let clients = sender::Clients {
        pooled: PooledClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap(),
        one_send: OneSendClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap(),
    };
    let base = crate::endpoint::Base::parse(&server.origin).unwrap();
    let (ctx, _) = RequestContext::new();
    let mut evidence = Evidence::new();
    Discovery::default()
        .require_with_evidence(
            Runtime {
                configured_credentials: None,
                client: &clients,
                ctx: &ctx,
                clock: &SystemClock,
                deadline: SystemClock.monotonic_now() + Duration::from_secs(5),
                sleeper: &Sleeper,
                jitter: &mut || 0,
            },
            &base,
            &Source,
            "han-1",
            ProjectRead::Id,
            &mut evidence,
        )
        .await
        .unwrap();
    assert!(evidence.output_reflects(&serde_json::json!("toy-token")));
}

#[tokio::test]
async fn short_base64url_credential_cannot_be_cached_as_project_id() {
    let server = Server::start(vec![response(
        "200 OK",
        br#"{"projects":[{"projectId":"prefixYT9iY2Qsuffix"}]}"#,
    )]);
    let discovery = Discovery::default();
    let source = RotatingSource(std::sync::Mutex::new("a?bcd"));
    let (ctx, _) = RequestContext::new();
    let deadline = SystemClock.monotonic_now() + Duration::from_secs(5);
    assert!(
        call_at_source(&discovery, &server, &ctx, &SystemClock, deadline, &source)
            .await
            .is_err()
    );
    assert!(
        discovery
            .project_id_context(&ctx, &SystemClock)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn credential_project_id_is_not_cached_after_rotation() {
    let server = Server::start(vec![
        response("200 OK", br#"{"projects":[{"projectId":"toy%2Dtoken"}]}"#),
        response("200 OK", br#"{"projects":[{"projectId":"safe-project"}]}"#),
    ]);
    let discovery = Discovery::default();
    let (ctx, _) = RequestContext::new();
    let source = RotatingSource(std::sync::Mutex::new("toy-token"));
    let deadline = SystemClock.monotonic_now() + Duration::from_secs(5);
    assert!(
        call_at_source(&discovery, &server, &ctx, &SystemClock, deadline, &source)
            .await
            .is_err()
    );
    assert!(
        discovery
            .project_id_context(&ctx, &SystemClock)
            .await
            .unwrap()
            .is_none()
    );
    *source.0.lock().unwrap() = "new-token";
    assert_eq!(
        call_at_source(&discovery, &server, &ctx, &SystemClock, deadline, &source)
            .await
            .unwrap()
            .id,
        "safe-project"
    );
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn selected_and_cached() {
    let server = Server::start(vec![response(
        "200 OK",
        br#"{"projects":[{"projectId":"one"}]}"#,
    )]);
    let discovery = Discovery::default();
    let (ctx, _) = RequestContext::new();
    assert_eq!(call(&discovery, &server, &ctx).await.unwrap().id, "one");
    assert_eq!(
        call(&discovery, &server, &ctx).await.unwrap().region,
        "han-1"
    );
    assert_eq!(server.requests(), 1);
    assert!(server.heads()[0].starts_with(b"GET /v1/projects "));
}

#[tokio::test]
async fn no_match_and_multiple_matches() {
    for (body, class) in [
        (
            br#"{"projects":[]}"#.as_slice(),
            ErrorClass::ProjectNotFound,
        ),
        (
            br#"{"projects":[{"projectId":"one"},{"projectId":"two"}]}"#,
            ErrorClass::ProjectAmbiguous,
        ),
    ] {
        let server = Server::start(vec![response("200 OK", body)]);
        let (ctx, _) = RequestContext::new();
        assert_eq!(
            call(&Discovery::default(), &server, &ctx)
                .await
                .unwrap_err()
                .class(),
            class
        );
    }
}

#[tokio::test]
async fn supplied_id_bypasses_request() {
    let server = Server::start(vec![]);
    let (ctx, _) = RequestContext::new();
    assert_eq!(
        call(
            &Discovery::new(Some("given".into()), "han-1"),
            &server,
            &ctx
        )
        .await
        .unwrap()
        .id,
        "given"
    );
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn concurrent_calls_share_read() {
    let server = Server::start(vec![response(
        "200 OK",
        br#"{"projects":[{"projectId":"one"}]}"#,
    )]);
    let discovery = Discovery::default();
    let (a, _) = RequestContext::new();
    let (b, _) = RequestContext::new();
    let (first, second) =
        tokio::join!(call(&discovery, &server, &a), call(&discovery, &server, &b));
    assert!(first.is_ok() && second.is_ok());
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn canceled_waiter_does_not_stop_holder() {
    let (started, ready) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::StallBeforeHead(started)]);
    let discovery = Discovery::default();
    let (holder, holder_cancel) = RequestContext::new();
    let (waiter, cancel) = RequestContext::new();
    let holder_future = call(&discovery, &server, &holder);
    tokio::pin!(holder_future);
    tokio::select! { _ = &mut holder_future => panic!("holder finished early"), _ = ready => {} }
    let waiter_future = call(&discovery, &server, &waiter);
    tokio::pin!(waiter_future);
    tokio::select! { _ = &mut waiter_future => panic!("waiter finished early"), _ = tokio::task::yield_now() => {} }
    cancel.cancel();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), waiter_future)
            .await
            .expect("waiter did not cancel")
            .unwrap_err()
            .class(),
        ErrorClass::Cancelled
    );
    assert_eq!(server.requests(), 1);
    tokio::select! { _ = &mut holder_future => panic!("holder stopped after waiter cancellation"), _ = tokio::task::yield_now() => {} }
    holder_cancel.cancel();
    assert_eq!(
        holder_future.await.unwrap_err().class(),
        ErrorClass::Cancelled
    );
}

#[tokio::test]
async fn canceled_holder_allows_waiter_to_take_over() {
    let (started, ready) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![
        Response::StallBeforeHead(started),
        response("200 OK", br#"{"projects":[{"projectId":"two"}]}"#),
    ]);
    let discovery = Discovery::default();
    let (holder, cancel) = RequestContext::new();
    let (waiter, _) = RequestContext::new();
    let holder_future = call(&discovery, &server, &holder);
    tokio::pin!(holder_future);
    tokio::select! { _ = &mut holder_future => panic!("holder finished early"), _ = ready => {} }
    let waiter_future = call(&discovery, &server, &waiter);
    tokio::pin!(waiter_future);
    tokio::select! { _ = &mut waiter_future => panic!("waiter finished early"), _ = tokio::task::yield_now() => {} }
    cancel.cancel();
    let (first, second) = tokio::join!(&mut holder_future, &mut waiter_future);
    assert_eq!(first.unwrap_err().class(), ErrorClass::Cancelled);
    assert_eq!(second.unwrap().id, "two");
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn runtime_deadline_expires_while_waiting_for_gate() {
    let server = Server::start(vec![]);
    let discovery = Discovery::default();
    let guard = discovery.selected.lock().await;
    let (ctx, _) = RequestContext::new();
    let clock = crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
    let deadline = clock.monotonic_now() + Duration::from_secs(1);
    let future = call_at(&discovery, &server, &ctx, &clock, deadline);
    tokio::pin!(future);
    tokio::select! { _ = &mut future => panic!("waiter finished early"), _ = tokio::task::yield_now() => {} }
    clock.advance(Duration::from_secs(1));
    let result = tokio::time::timeout(Duration::from_secs(1), future).await;
    assert!(result.is_ok(), "waiter did not honor deadline");
    assert_eq!(
        result.unwrap().unwrap_err().class(),
        ErrorClass::DeadlineExceeded
    );
    drop(guard);
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn runtime_deadline_crossed_before_publish_leaves_cache_empty() {
    let server = Server::start(vec![response(
        "200 OK",
        br#"{"projects":[{"projectId":"one"}]}"#,
    )]);
    let clock = crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
    let deadline = clock.monotonic_now() + Duration::from_secs(1);
    let advanced = clock.clone();
    let discovery = Discovery {
        selected: Mutex::new(None),
        before_publish: Some(Box::new(move || advanced.advance(Duration::from_secs(1)))),
    };
    let (ctx, _) = RequestContext::new();
    let result = call_at(&discovery, &server, &ctx, &clock, deadline).await;
    assert_eq!(result.unwrap_err().class(), ErrorClass::DeadlineExceeded);
    assert!(discovery.selected.lock().await.is_none());
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn failed_read_is_not_cached() {
    let server = Server::start(vec![
        response("403 Forbidden", b"{}"),
        response("200 OK", br#"{"projects":[{"projectId":"two"}]}"#),
    ]);
    let discovery = Discovery::default();
    let (ctx, _) = RequestContext::new();
    assert!(call(&discovery, &server, &ctx).await.is_err());
    assert_eq!(call(&discovery, &server, &ctx).await.unwrap().id, "two");
    assert_eq!(server.requests(), 2);
}
