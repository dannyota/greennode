use super::*;
use crate::wire::{
    connection::Trust,
    test_server::{Response, Server},
};
use crate::{RequestContext, TestClock};

async fn attempt(
    status: &str,
    body: &[u8],
    headers: Vec<(String, String)>,
) -> (Server, Result<Token, Box<crate::Error>>) {
    let mut headers = headers;
    headers.push(("Content-Length".into(), body.len().to_string()));
    let server = Server::start(vec![Response::Http {
        status: status.into(),
        headers,
        body: body.to_vec(),
    }]);
    let clock = TestClock::new(OffsetDateTime::UNIX_EPOCH);
    let (context, _) = RequestContext::new();
    let result = exchange_at(
        "client",
        &Secret::new("canary-secret"),
        &format!("{}/accounts-api/v2/auth/token", server.origin),
        Trust::test(server.root_der.clone()),
        &context,
        &clock,
        Duration::ZERO,
    )
    .await;
    (server, result)
}

#[tokio::test]
async fn unauthorized_stalled_body_returns_at_head() {
    let (sent, ready) = tokio::sync::oneshot::channel();
    let (eof, closed) = tokio::sync::oneshot::channel();
    let head = b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 1\r\n\r\n".to_vec();
    let server = Server::start(vec![Response::ObserveStall { head, sent, eof }]);
    let clock = TestClock::new(OffsetDateTime::UNIX_EPOCH);
    let (context, _) = RequestContext::new();
    let secret = Secret::new("toy");
    let future = exchange_at(
        "client",
        &secret,
        &server.origin,
        Trust::test(server.root_der.clone()),
        &context,
        &clock,
        Duration::ZERO,
    );
    let result = tokio::time::timeout(Duration::from_secs(60), future).await;
    assert!(result.is_ok());
    assert!(ready.await.is_ok());
    let error = result.unwrap().err().unwrap();
    let detail = error.service_account_login_error().unwrap();
    assert_eq!(detail.reason(), "token endpoint rejected request");
    assert_eq!(detail.status(), Some(401));
    assert!(
        tokio::time::timeout(Duration::from_secs(60), closed)
            .await
            .unwrap()
            .unwrap()
    );
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn caller_cancellation_identity() {
    let server = Server::start(vec![]);
    let clock = TestClock::new(OffsetDateTime::UNIX_EPOCH);
    let (context, handle) = RequestContext::new();
    handle.cancel();
    let result = exchange_at(
        "client",
        &Secret::new("toy"),
        &server.origin,
        Trust::test(server.root_der.clone()),
        &context,
        &clock,
        Duration::ZERO,
    )
    .await;
    let error = result.err().unwrap();
    assert!(error.has_identity(crate::ErrorClass::Cancelled));
    assert_eq!(
        error.service_account_login_error().unwrap().reason(),
        "token request failed"
    );
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn caller_deadline_identity() {
    let server = Server::start(vec![]);
    let clock = TestClock::new(OffsetDateTime::UNIX_EPOCH);
    let (context, _) = RequestContext::new();
    let context = context.child_with_deadline(clock.monotonic_now());
    let result = exchange_at(
        "client",
        &Secret::new("toy"),
        &server.origin,
        Trust::test(server.root_der.clone()),
        &context,
        &clock,
        Duration::ZERO,
    )
    .await;
    let error = result.err().unwrap();
    assert!(error.has_identity(crate::ErrorClass::DeadlineExceeded));
    assert_eq!(
        error.service_account_login_error().unwrap().reason(),
        "token request failed"
    );
    assert_eq!(server.requests(), 0);
}

struct BodyClock {
    inner: TestClock,
    waits: std::sync::atomic::AtomicUsize,
    body: tokio::sync::Notify,
}

impl Clock for BodyClock {
    fn utc_now(&self) -> OffsetDateTime {
        self.inner.utc_now()
    }
    fn monotonic_now(&self) -> tokio::time::Instant {
        self.inner.monotonic_now()
    }
    fn sleep_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        if self.waits.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            self.body.notify_one();
        }
        self.inner.sleep_until(deadline)
    }
}

#[tokio::test]
async fn cancellation_during_body_keeps_identity_and_status() {
    let (sent, received) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::Stall(sent, 100)]);
    let clock = BodyClock {
        inner: TestClock::new(OffsetDateTime::UNIX_EPOCH),
        waits: std::sync::atomic::AtomicUsize::new(0),
        body: tokio::sync::Notify::new(),
    };
    let (context, handle) = RequestContext::new();
    let secret = Secret::new("toy");
    let future = exchange_at(
        "client",
        &secret,
        &server.origin,
        Trust::test(server.root_der.clone()),
        &context,
        &clock,
        Duration::ZERO,
    );
    tokio::pin!(future);
    tokio::select! {
        result = &mut future => panic!("unexpected early response: {}", result.is_ok()),
        _ = clock.body.notified() => {}
    }
    assert!(received.await.is_ok());
    handle.cancel();
    let finished = tokio::time::timeout(Duration::from_secs(60), &mut future).await;
    assert!(finished.is_ok());
    let error = finished.unwrap().err().unwrap();
    assert!(error.has_identity(ErrorClass::Cancelled));
    let detail = error.service_account_login_error().unwrap();
    assert_eq!(detail.reason(), "invalid token response");
    assert_eq!(detail.status(), Some(200));
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn configured_timeout_during_body() {
    let (sent, received) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::Stall(sent, 100)]);
    let clock = BodyClock {
        inner: TestClock::new(OffsetDateTime::UNIX_EPOCH),
        waits: std::sync::atomic::AtomicUsize::new(0),
        body: tokio::sync::Notify::new(),
    };
    let (context, _) = RequestContext::new();
    let secret = Secret::new("toy");
    let future = exchange_at(
        "client",
        &secret,
        &server.origin,
        Trust::test(server.root_der.clone()),
        &context,
        &clock,
        Duration::from_secs(5),
    );
    tokio::pin!(future);
    tokio::select! {
        result = &mut future => panic!("unexpected early response: {}", result.is_ok()),
        _ = clock.body.notified() => {}
    }
    assert!(received.await.is_ok());
    clock.inner.advance(Duration::from_secs(5));
    let finished = tokio::time::timeout(Duration::from_secs(60), &mut future).await;
    assert!(finished.is_ok());
    let error = finished.unwrap().err().unwrap();
    assert!(error.has_identity(ErrorClass::Auth));
    assert!(!error.has_identity(ErrorClass::DeadlineExceeded));
    let detail = error.service_account_login_error().unwrap();
    assert_eq!(detail.reason(), "invalid token response");
    assert_eq!(detail.status(), Some(200));
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn zero_timeout_uses_default() {
    use std::future::Future;
    let (sent, received) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::Stall(sent, 100)]);
    let clock = BodyClock {
        inner: TestClock::new(OffsetDateTime::UNIX_EPOCH),
        waits: std::sync::atomic::AtomicUsize::new(0),
        body: tokio::sync::Notify::new(),
    };
    let (context, _) = RequestContext::new();
    let secret = Secret::new("toy");
    let future = exchange_at(
        "client",
        &secret,
        &server.origin,
        Trust::test(server.root_der.clone()),
        &context,
        &clock,
        Duration::ZERO,
    );
    tokio::pin!(future);
    tokio::select! {
        result = &mut future => panic!("unexpected early response: {}", result.is_ok()),
        _ = clock.body.notified() => {}
    }
    assert!(received.await.is_ok());
    clock.inner.advance(Duration::from_secs(119));
    let pending =
        std::future::poll_fn(|cx| std::task::Poll::Ready(future.as_mut().poll(cx).is_pending()))
            .await;
    assert!(pending);
    clock.inner.advance(Duration::from_secs(1));
    let result = tokio::time::timeout(Duration::from_secs(60), &mut future).await;
    assert!(result.is_ok());
    let error = result.unwrap().err().unwrap();
    assert!(error.has_identity(ErrorClass::Auth));
    let detail = error.service_account_login_error().unwrap();
    assert_eq!(detail.reason(), "invalid token response");
    assert_eq!(detail.status(), Some(200));
}

struct BackwardClock(TestClock);

impl Clock for BackwardClock {
    fn utc_now(&self) -> OffsetDateTime {
        OffsetDateTime::UNIX_EPOCH - time::Duration::seconds(60)
    }
    fn monotonic_now(&self) -> tokio::time::Instant {
        self.0.monotonic_now()
    }
    fn sleep_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        self.0.sleep_until(deadline)
    }
}

#[test]
fn freshness_uses_monotonic_time_when_wall_clock_moves_back() {
    let clock = BackwardClock(TestClock::new(OffsetDateTime::UNIX_EPOCH));
    let body = br#"{"access_token":"toy","expires_in":31}"#;
    let started = clock.monotonic_now() - Duration::from_secs(2);
    assert!(decode_at_clock(body, OffsetDateTime::UNIX_EPOCH, started, &clock).is_none());
    let token = decode_at_clock(
        body,
        OffsetDateTime::UNIX_EPOCH,
        clock.monotonic_now(),
        &clock,
    )
    .unwrap();
    assert_eq!(
        token.expires_at,
        Some(OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(31))
    );
}

#[test]
fn rejects_private_number_object() {
    let body = br#"{"access_token":"toy","expires_in":{"$serde_json::private::Number":"31"}}"#;
    let rejected = decode(body, OffsetDateTime::UNIX_EPOCH, Duration::ZERO).is_none();
    assert!(rejected);
}

#[test]
fn accepts_bounded_ignored_nesting() {
    let body = format!(
        "{{\"access_token\":\"toy\",\"expires_in\":31,\"ignored\":{}0{}}}",
        "[".repeat(127),
        "]".repeat(127)
    );
    assert!(decode(body.as_bytes(), OffsetDateTime::UNIX_EPOCH, Duration::ZERO).is_some());
    let body = format!(
        "{{\"access_token\":\"toy\",\"expires_in\":31,\"ignored\":{}0{}}}",
        "[".repeat(128),
        "]".repeat(128)
    );
    assert!(decode(body.as_bytes(), OffsetDateTime::UNIX_EPOCH, Duration::ZERO).is_none());
}

#[tokio::test]
async fn unauthorized() {
    let (server, result) = attempt("401 Unauthorized", b"canary-secret", vec![]).await;
    assert!(result.is_err());
    let error = result.err().unwrap();
    let detail = error.service_account_login_error().unwrap();
    assert_eq!(detail.reason(), "token endpoint rejected request");
    assert_eq!(detail.status(), Some(401));
    let error_safe = !format!("{error:?}").contains("canary-secret");
    assert!(error_safe);
    let wrapped = error;
    let error_safe = !format!("{wrapped:?} {wrapped}").contains("canary-secret");
    assert!(error_safe);
    assert_eq!(server.requests(), 1);
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn server_error() {
    let (server, result) = attempt("503 Unavailable", b"canary-secret", vec![]).await;
    assert!(result.is_err());
    let error = result.err().unwrap();
    let detail = error.service_account_login_error().unwrap();
    assert_eq!(detail.reason(), "token endpoint rejected request");
    assert_eq!(detail.status(), Some(503));
    assert_eq!(server.requests(), 1);
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn redirect() {
    let location = "/redirect-canary-secret";
    let (server, result) =
        attempt("302 Found", b"", vec![("Location".into(), location.into())]).await;
    assert!(result.is_err());
    let error = result.err().unwrap();
    let detail = error.service_account_login_error().unwrap();
    assert_eq!(detail.reason(), "token endpoint redirected request");
    assert_eq!(detail.status(), Some(302));
    assert!(!format!("{error:?} {}", detail.reason()).contains(location));
    assert_eq!(server.requests(), 1);
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn valid_exchange() {
    let (server, result) = attempt(
        "200 OK",
        br#"{"access_token":"toy-token","expires_in":1800,"token_type":"bEaReR"}"#,
        vec![],
    )
    .await;
    let token = result.unwrap();
    assert_eq!(
        token.expires_at,
        Some(OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1800))
    );
    let token_matches = token.access_token.expose_secret() == "toy-token";
    assert!(token_matches);
    let token_safe = !format!("{token:?}").contains("toy-token");
    assert!(token_safe);
    assert_eq!(server.requests(), 1);
    assert_eq!(server.heads().len(), 1);
    let bodies = server.bodies();
    let body_matches = bodies.len() == 1 && bodies[0] == br#"{"grant_type":"client_credentials"}"#;
    assert!(body_matches);
    let heads = server.heads();
    let Ok(raw_head) = String::from_utf8(heads[0].clone()) else {
        panic!("invalid request head");
    };
    let authorization_matches = raw_head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .any(|(name, value)| {
            name.eq_ignore_ascii_case("authorization")
                && value.trim() == "Basic Y2xpZW50OmNhbmFyeS1zZWNyZXQ="
        });
    assert!(authorization_matches);
    let head = raw_head.to_ascii_lowercase();
    assert!(head.starts_with("post /accounts-api/v2/auth/token http/1.1\r\n"));
    assert!(head.contains("content-type: application/json\r\n"));
    assert!(head.contains("accept: application/json\r\n"));
    assert!(head.contains("accept-encoding: gzip\r\n"));
    assert!(!head.contains("cookie:"));
}

#[tokio::test]
async fn gzip_response() {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(br#"{"access_token":"toy-token","expires_in":31}"#)
        .unwrap();
    let encoded = encoder.finish().unwrap();
    let (server, result) = attempt(
        "200 OK",
        &encoded,
        vec![("Content-Encoding".into(), "gzip".into())],
    )
    .await;
    assert!(result.is_ok());
    assert_eq!(server.requests(), 1);
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn oversized_rejection_keeps_status() {
    let body = vec![b'x'; 65537];
    let (server, result) = attempt("401 Unauthorized", &body, vec![]).await;
    assert!(result.is_err());
    let error = result.err().unwrap();
    let detail = error.service_account_login_error().unwrap();
    assert_eq!(detail.reason(), "token endpoint rejected request");
    assert_eq!(detail.status(), Some(401));
    assert_eq!(server.requests(), 1);
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn oversized_response() {
    let valid = br#"{"access_token":"toy","expires_in":1800}"#;
    let mut body = valid.to_vec();
    body.resize(65537, b' ');
    let (server, result) = attempt("200 OK", &body, vec![]).await;
    assert!(result.is_err());
    let error = result.err().unwrap();
    let detail = error.service_account_login_error().unwrap();
    assert_eq!(detail.reason(), "invalid token response");
    assert_eq!(detail.status(), Some(200));
    assert_eq!(server.requests(), 1);
    assert_eq!(server.heads().len(), 1);
}

macro_rules! invalid {
    ($name:ident, $body:expr) => {
        #[tokio::test]
        async fn $name() {
            let (server, result) = attempt("200 OK", $body.as_bytes(), vec![]).await;
            assert!(result.is_err());
            let error = result.err().unwrap();
            let detail = error.service_account_login_error().unwrap();
            assert_eq!(detail.reason(), "invalid token response");
            assert_eq!(detail.status(), Some(200));
            let error_safe = !format!("{error:?} {}", detail.reason()).contains("canary-secret");
            assert!(error_safe);
            assert_eq!(server.requests(), 1);
            assert_eq!(server.heads().len(), 1);
        }
    };
}

invalid!(null, "null");
invalid!(sequence_token_fields, r#"["toy",31]"#);
invalid!(array, "[]");
invalid!(missing, "{}");
invalid!(short_expiry, r#"{"access_token":"toy","expires_in":30}"#);
invalid!(zero_expiry, r#"{"access_token":"toy","expires_in":0}"#);
invalid!(negative_expiry, r#"{"access_token":"toy","expires_in":-1}"#);
invalid!(
    fractional_expiry,
    r#"{"access_token":"toy","expires_in":31.0}"#
);
invalid!(
    string_expiry,
    r#"{"access_token":"toy","expires_in":"1800"}"#
);
invalid!(null_expiry, r#"{"access_token":"toy","expires_in":null}"#);
invalid!(null_token, r#"{"access_token":null,"expires_in":1800}"#);
invalid!(number_token, r#"{"access_token":1,"expires_in":1800}"#);
invalid!(empty_token, r#"{"access_token":"","expires_in":1800}"#);
invalid!(space_token, r#"{"access_token":"a b","expires_in":1800}"#);
invalid!(
    control_token,
    "{\"access_token\":\"a\\u0000b\",\"expires_in\":1800}"
);
invalid!(
    null_type,
    r#"{"access_token":"toy","expires_in":1800,"token_type":null}"#
);
invalid!(
    bad_type,
    r#"{"access_token":"toy","expires_in":1800,"token_type":"MAC"}"#
);
invalid!(
    number_type,
    r#"{"access_token":"toy","expires_in":1800,"token_type":3}"#
);
invalid!(trailing, r#"{"access_token":"toy","expires_in":1800} {}"#);
invalid!(
    duplicate_token,
    r#"{"access_token":"toy","access_token":"other","expires_in":1800}"#
);
invalid!(
    duplicate_expiry,
    r#"{"access_token":"toy","expires_in":1800,"expires_in":1801}"#
);
invalid!(
    duplicate_type,
    r#"{"access_token":"toy","expires_in":1800,"token_type":"bearer","token_type":"Bearer"}"#
);
invalid!(
    duration_overflow,
    r#"{"access_token":"toy","expires_in":9223372037}"#
);
invalid!(
    integer_overflow,
    r#"{"access_token":"toy","expires_in":9223372036854775808}"#
);
invalid!(truncated, r#"{"access_token":"toy","expires_in":1800"#);
