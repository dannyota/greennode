use super::*;
use crate::{ErrorClass, WriteDispatch};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

#[tokio::test]
async fn truncated_body_keeps_only_opted_in_status() {
    for keep_status in [false, true] {
        let server = Server::start(vec![Response::RawClose(
            b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 5\r\n\r\n{}".to_vec(),
        )]);
        let op = Operation {
            keep_status,
            ..declaration()
        };
        let failure = call(&server, &op, &["one"]).await.unwrap_err();
        let error = failure.error();
        let api = error.api_error().unwrap();
        assert_eq!(error.class(), ErrorClass::Api);
        assert_eq!(api.status(), keep_status.then_some(503));
        assert_eq!(
            error
                .detail::<crate::HttpStatus>()
                .map(|status| status.as_u16()),
            keep_status.then_some(503)
        );
        assert_eq!(api.code(), if keep_status { "ServerError" } else { "" });
        assert_eq!(
            api.go_text(),
            if keep_status {
                "test.get: request failed (status 503)"
            } else {
                "test.get: request failed: network request failed"
            }
        );
        assert!(!api.retryable());
    }
}

#[tokio::test]
async fn cross_host_redirect_drops_status_even_when_body_failures_keep_it() {
    let body = br#"{"code":"Moved","message":"moved elsewhere"}"#.to_vec();
    let redirect = || Response::Http {
        status: "302 Found".into(),
        headers: vec![
            ("Location".into(), "https://localhost:1/hidden".into()),
            ("Content-Length".into(), body.len().to_string()),
        ],
        body: body.clone(),
    };
    let server = Server::start((0..4).map(|_| redirect()).collect());
    let op = Operation {
        keep_status: true,
        retry: redirect::Retry::Repeatable,
        ..declaration()
    };
    let failure = call(&server, &op, &["one"]).await.unwrap_err();
    let error = failure.error();
    let api = error.api_error().unwrap();
    assert_eq!(error.class(), ErrorClass::Api);
    assert_eq!(api.status(), None);
    assert_eq!(error.detail::<StatusCode>(), None);
    assert_eq!(api.code(), "");
    assert_eq!(
        api.go_text(),
        format!(
            "test.get: request failed: redirected from {:?} to \"localhost:1\": cross-host redirect refused",
            server.origin.trim_start_matches("https://")
        )
    );
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn local_reflection_is_invalid_input_before_send() {
    let server = Server::start(vec![]);
    let op = Operation {
        class: redirect::Classification::Write,
        ..declaration()
    };
    let failure = call(&server, &op, &["toy-token"]).await.unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn raw_write_overflow_has_no_api_status_and_may_have_been_sent() {
    let server = Server::start(vec![response("404 Not Found", b"0123456789")]);
    let operation = Operation {
        class: redirect::Classification::Write,
        raw_body: true,
        cap: 4,
        ..declaration()
    };
    let failure = call(&server, &operation, &["one"]).await.unwrap_err();
    let error = failure.error();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(error.is_body_too_large());
    assert!(!error.has_class(ErrorClass::NotFound));
    assert_eq!(error.api_error().unwrap().status(), None);
    assert_eq!(error.detail::<StatusCode>(), None);
}

#[tokio::test]
async fn failed_dial_reports_safe_go_cause() {
    let server = Server::start(vec![]);
    let base = crate::endpoint::Base::parse(&server.origin).unwrap();
    let clients = clients(&server);
    drop(server);
    let (ctx, _) = crate::RequestContext::new();
    let clock = TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
    let op = Operation {
        class: redirect::Classification::Write,
        ..declaration()
    };
    let failure = run(
        &clients,
        &ctx,
        &clock,
        &base,
        &op,
        RouteAuth::Bearer(&Source(false, "toy-token")),
    )
    .await
    .err()
    .unwrap();
    let api = failure.error().api_error().unwrap();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(api.status(), None);
    assert!(api.retryable());
    assert_eq!(api.message(), "");
    assert_eq!(
        api.go_text(),
        "test.get: request failed: dial: connect: connection refused"
    );
}

#[tokio::test]
async fn cancelled_before_head_reports_safe_go_cause() {
    let (sent, ready) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::StallBeforeHead(sent)]);
    let base = crate::endpoint::Base::parse(&server.origin).unwrap();
    let clients = clients(&server);
    let (ctx, cancel) = crate::RequestContext::new();
    let clock = TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
    let op = declaration();
    let cancel = async {
        ready.await.unwrap();
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(
        run(&clients, &ctx, &clock, &base, &op, RouteAuth::PublicHtml),
        cancel
    );
    let failure = result.err().unwrap();
    let error = failure.error();
    assert_eq!(error.class(), ErrorClass::Cancelled);
    let api = error.api_error().unwrap();
    assert_eq!(api.status(), None);
    assert_eq!(api.go_text(), "test.get: request failed: canceled");
}

#[tokio::test]
async fn deadline_during_body_reports_timeout_and_keeps_opted_in_status() {
    for keep_status in [false, true] {
        let (sent, _) = tokio::sync::oneshot::channel();
        let server = Server::start(vec![Response::Stall(sent, 5)]);
        let base = crate::endpoint::Base::parse(&server.origin).unwrap();
        let clients = clients(&server);
        let (ctx, _) = crate::RequestContext::new();
        let clock = crate::testing::ResponseBodyClock::new();
        let op = Operation {
            keep_status,
            ..declaration()
        };
        let advance = async {
            clock.started().await;
            clock.advance(Duration::from_secs(5));
        };
        let (result, ()) = tokio::join!(
            run(&clients, &ctx, &clock, &base, &op, RouteAuth::PublicHtml),
            advance
        );
        let failure = result.err().unwrap();
        let error = failure.error();
        assert_eq!(error.class(), ErrorClass::DeadlineExceeded);
        let api = error.api_error().unwrap();
        assert_eq!(api.status(), keep_status.then_some(200));
        assert_eq!(
            api.go_text(),
            if keep_status {
                "test.get: request failed (status 200)"
            } else {
                "test.get: request failed: timed out"
            }
        );
    }
}

fn clients(server: &Server) -> sender::Clients {
    sender::Clients {
        pooled: PooledClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap(),
        one_send: OneSendClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap(),
    }
}

#[allow(clippy::result_large_err)]
async fn run(
    clients: &sender::Clients,
    ctx: &crate::RequestContext,
    clock: &dyn Clock,
    base: &crate::endpoint::Base,
    op: &Operation,
    auth: RouteAuth<'_>,
) -> Result<sender::Response, crate::Failure<()>> {
    execute_raw(
        sender::Runtime {
            configured_credentials: None,
            client: clients,
            ctx,
            clock,
            deadline: clock.monotonic_now() + Duration::from_secs(5),
            sleeper: &Immediate,
            jitter: &mut || 0,
        },
        base,
        op,
        &Options::DEFAULT,
        &["one"],
        &[],
        auth,
        bytes::Bytes::new(),
        &mut Evidence::new(),
    )
    .await
}

#[tokio::test]
async fn truncated_redirect_is_refused_at_the_head() {
    for location in ["https://localhost:1/hidden", "/bad%zz"] {
        let server = Server::start(vec![Response::RawClose(
            format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 8\r\n\r\n{{}}")
                .into_bytes(),
        )]);
        let op = Operation {
            keep_status: true,
            retry: redirect::Retry::Repeatable,
            ..declaration()
        };
        let failure = call(&server, &op, &["one"]).await.unwrap_err();
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.status(), None);
        assert!(failure.error().detail::<BodyReadFailure>().is_none());
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn truncated_unauthorized_invalidates_only_when_status_is_kept() {
    for keep_status in [false, true] {
        let server = Server::start(vec![Response::RawClose(
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 8\r\n\r\n{}".to_vec(),
        )]);
        let source = RefreshSource {
            tokens: AtomicUsize::new(0),
            invalidations: AtomicUsize::new(0),
            fail_refresh: false,
        };
        let op = Operation {
            keep_status,
            ..declaration()
        };
        let _ = call_with_source(&server, &op, &["one"], &[], bytes::Bytes::new(), &source)
            .await
            .unwrap_err();
        assert_eq!(
            source.invalidations.load(Ordering::SeqCst),
            usize::from(keep_status)
        );
        assert_eq!(source.tokens.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn refresh_failure_preserves_its_own_class_and_drops_the_rejected_status() {
    let server = Server::start(vec![response("401 Unauthorized", b"{}")]);
    let source = RefreshSource {
        tokens: AtomicUsize::new(0),
        invalidations: AtomicUsize::new(0),
        fail_refresh: true,
    };
    let op = Operation {
        keep_status: true,
        retry: redirect::Retry::Repeatable,
        allow_recovery: true,
        ..declaration()
    };
    let failure = call_with_source(&server, &op, &["one"], &[], bytes::Bytes::new(), &source)
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::TokenCache);
    assert!(failure.error().api_error().is_none());
    assert_eq!(source.invalidations.load(Ordering::SeqCst), 1);
    assert_eq!(source.tokens.load(Ordering::SeqCst), 2);
    assert_eq!(server.requests(), 1);
}

struct RefreshSource {
    tokens: AtomicUsize,
    invalidations: AtomicUsize,
    fail_refresh: bool,
}

impl sender::BearerSource for RefreshSource {
    fn token<'a>(
        &'a self,
        _: &'a crate::RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<sender::BearerToken, crate::Error>> + Send + 'a>> {
        Box::pin(async move {
            if self.tokens.fetch_add(1, Ordering::SeqCst) > 0 && self.fail_refresh {
                return Err(crate::Error::new(ErrorClass::TokenCache));
            }
            Ok(sender::BearerToken {
                token: crate::Token::new(crate::Secret::new("toy-token"), None),
                obtained_at: None,
                signin_credentials: None,
            })
        })
    }

    fn rejected<'a>(
        &'a self,
        _: &'a crate::RequestContext,
        _: &'a crate::Secret,
        _: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
        Box::pin(async move {
            self.invalidations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

#[tokio::test]
async fn reset_body_frame_retains_the_safe_read_phase_cause() {
    let (reset, receive) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::ResetResponseBody {
        head: b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n".to_vec(),
        reset: receive,
    }]);
    let base = crate::endpoint::Base::parse(&server.origin).unwrap();
    let clients = clients(&server);
    let (ctx, _) = crate::RequestContext::new();
    let clock = crate::testing::ResponseBodyClock::new();
    let op = declaration();
    let abort = async {
        clock.started().await;
        reset.send(()).unwrap();
    };
    let (result, ()) = tokio::join!(
        run(&clients, &ctx, &clock, &base, &op, RouteAuth::PublicHtml),
        abort
    );
    let failure = result.err().unwrap();
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.status(), None);
    assert_eq!(
        api.go_text(),
        "test.get: request failed: read: connection reset"
    );
}

#[tokio::test]
async fn cancelled_or_expired_once_invalidation_keeps_unauthorized_status() {
    for expired in [false, true] {
        let server = Server::start(vec![response("401 Unauthorized", b"{}")]);
        let base = crate::endpoint::Base::parse(&server.origin).unwrap();
        let clients = clients(&server);
        let (ctx, cancel) = crate::RequestContext::new();
        let clock = TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
        let source = PendingInvalidation(Notify::new());
        let op = Operation {
            id: "test.write",
            method: Method::POST,
            class: redirect::Classification::Write,
            keep_status: true,
            ..declaration()
        };
        let end = async {
            source.0.notified().await;
            if expired {
                clock.advance(Duration::from_secs(5));
            } else {
                cancel.cancel();
            }
        };
        let (result, ()) = tokio::join!(
            run(
                &clients,
                &ctx,
                &clock,
                &base,
                &op,
                RouteAuth::Bearer(&source)
            ),
            end
        );
        let failure = result.err().unwrap();
        let error = failure.error();
        let api = error.api_error().unwrap();
        assert_eq!((api.status(), api.code()), (Some(401), "Unauthorized"));
        assert!(error.has_class(if expired {
            ErrorClass::DeadlineExceeded
        } else {
            ErrorClass::Cancelled
        }));
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert_eq!(server.requests(), 1);
    }
}

struct PendingInvalidation(Notify);

impl sender::BearerSource for PendingInvalidation {
    fn token<'a>(
        &'a self,
        ctx: &'a crate::RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<sender::BearerToken, crate::Error>> + Send + 'a>> {
        static SOURCE: Source = Source(false, "toy-token");
        SOURCE.token(ctx)
    }

    fn rejected<'a>(
        &'a self,
        _: &'a crate::RequestContext,
        _: &'a crate::Secret,
        _: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
        Box::pin(async move {
            self.0.notify_one();
            std::future::pending().await
        })
    }
}

#[tokio::test]
async fn empty_location_returns_the_complete_redirect_response() {
    let body = br#"{"code":"Moved","message":"destination omitted"}"#.to_vec();
    let server = Server::start(vec![Response::Http {
        status: "302 Found".into(),
        headers: vec![
            ("Location".into(), String::new()),
            ("Content-Length".into(), body.len().to_string()),
        ],
        body,
    }]);
    let op = Operation {
        retry: redirect::Retry::Repeatable,
        ..declaration()
    };
    let failure = call(&server, &op, &["one"]).await.unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(
        (api.status(), api.code(), api.message()),
        (Some(302), "Moved", "destination omitted")
    );
    assert_eq!(server.requests(), 1);
}
