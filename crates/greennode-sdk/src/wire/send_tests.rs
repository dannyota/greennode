use super::{
    connection::{self, HeadLimit, Trust},
    send::{self, Encoding, Error},
    test_server::{Response, Server},
};
use crate::{Clock, TestClock};
use bytes::Bytes;
use http::{Method, Request};
use http_body_util::Full;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[derive(Clone)]
struct SignalingClock {
    inner: TestClock,
    sleeps: Arc<AtomicUsize>,
    head: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    silent: bool,
}

impl Clock for SignalingClock {
    fn utc_now(&self) -> time::OffsetDateTime {
        self.inner.utc_now()
    }
    fn monotonic_now(&self) -> tokio::time::Instant {
        self.inner.monotonic_now()
    }
    fn sleep_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        if self.sleeps.fetch_add(1, Ordering::SeqCst) == 1
            && let Some(head) = self.head.lock().unwrap().take()
        {
            let _ = head.send(());
        }
        if self.silent {
            Box::pin(std::future::pending())
        } else {
            self.inner.sleep_until(deadline)
        }
    }
}

struct BoundaryClock {
    start: tokio::time::Instant,
    sleeps: AtomicUsize,
    checks: AtomicUsize,
    threshold: usize,
}

impl Clock for BoundaryClock {
    fn utc_now(&self) -> time::OffsetDateTime {
        time::OffsetDateTime::UNIX_EPOCH
    }
    fn monotonic_now(&self) -> tokio::time::Instant {
        if self.sleeps.load(Ordering::SeqCst) >= 2
            && self.checks.fetch_add(1, Ordering::SeqCst) >= self.threshold
        {
            self.start + Duration::from_secs(1)
        } else {
            self.start
        }
    }
    fn sleep_until(
        &self,
        _: tokio::time::Instant,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.sleeps.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::pending())
    }
}

fn client(server: &Server) -> connection::Client {
    connection::client(false, HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap()
}

fn req(method: Method, uri: &str) -> Request<Full<Bytes>> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Full::new(Bytes::new()))
        .unwrap()
}

fn response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Response {
    Response::Http {
        status: status.into(),
        headers: headers
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
        body: body.to_vec(),
    }
}

async fn send(
    client: &connection::Client,
    request: Request<Full<Bytes>>,
    cap: usize,
) -> Result<send::Reply, Error> {
    let clock = crate::SystemClock;
    send::once(
        client,
        request,
        cap,
        Encoding::Identity,
        &clock,
        clock.monotonic_now() + Duration::from_secs(5),
    )
    .await
}

mod gzip_tests;
mod policy_tests;

#[test]
fn send_errors_map_to_retry_outcomes() {
    use super::product::retry::{Outcome, SendFailure};
    assert!(matches!(
        Error::Transport(SendFailure::FailedDial).retry_outcome(),
        Some(Outcome::Send(SendFailure::FailedDial))
    ));
    assert!(matches!(
        Error::Transport(SendFailure::PossiblySent).retry_outcome(),
        Some(Outcome::Send(SendFailure::PossiblySent))
    ));
    assert!(Error::Deadline.retry_outcome().is_none());
}

#[tokio::test]
async fn refused_port_is_proven_dial_failure() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let client = connection::client(false, HeadLimit::Api, Trust::webpki()).unwrap();
    let result = send(
        &client,
        req(Method::GET, &format!("https://{address}/")),
        10,
    )
    .await;
    assert!(matches!(
        result,
        Err(Error::Transport(
            super::product::retry::SendFailure::FailedDial
        ))
    ));
}

#[tokio::test]
async fn untrusted_tls_is_possibly_sent() {
    let server = Server::start(vec![]);
    let client = connection::client(false, HeadLimit::Api, Trust::webpki()).unwrap();
    let result = send(&client, req(Method::GET, &server.origin), 10).await;
    assert!(matches!(
        result,
        Err(Error::Transport(
            super::product::retry::SendFailure::PossiblySent
        ))
    ));
}

#[tokio::test]
async fn close_after_head_is_possibly_sent() {
    let server = Server::start(vec![Response::Close]);
    let result = send(&client(&server), req(Method::GET, &server.origin), 10).await;
    assert!(matches!(
        result,
        Err(Error::Transport(
            super::product::retry::SendFailure::PossiblySent
        ))
    ));
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn reset_during_request_body_is_possibly_sent() {
    let (seen, received) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::ResetMidBody(seen)]);
    let request = Request::builder()
        .method(Method::POST)
        .uri(&server.origin)
        .body(Full::new(Bytes::from(vec![b'x'; 8 * 1024 * 1024])))
        .unwrap();
    let result = send(&client(&server), request, 10).await;
    assert!(received.await.is_ok());
    assert!(matches!(
        result,
        Err(Error::Transport(
            super::product::retry::SendFailure::PossiblySent
        ))
    ));
}

#[tokio::test]
async fn body_cap_accepts_exact_and_refuses_one_more() {
    let server = Server::start(vec![
        Response::Raw(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n".to_vec(),
        ),
        Response::Raw(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nabcd\r\n0\r\n\r\n".to_vec(),
        ),
    ]);
    let client = client(&server);
    let reply = send(&client, req(Method::GET, &server.origin), 3)
        .await
        .unwrap();
    assert_eq!(reply.body, b"abc");
    assert!(matches!(
        send(&client, req(Method::GET, &server.origin), 3).await,
        Err(error) if error.kind() == &Error::BodyTooLarge
    ));
}

#[tokio::test]
async fn fragmented_cap_plus_one_with_trailers_is_refused() {
    let (first_tx, first_rx) = tokio::sync::oneshot::channel();
    let (go_tx, go_rx) = tokio::sync::oneshot::channel();
    let (last_tx, last_rx) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::Fragment {
        first: first_tx,
        proceed: go_rx,
        last: last_tx,
    }]);
    let client = client(&server);
    let uri = server.origin.clone();
    let worker = tokio::spawn(async move { send(&client, req(Method::GET, &uri), 3).await });
    tokio::time::timeout(Duration::from_secs(3), first_rx)
        .await
        .unwrap()
        .unwrap();
    go_tx.send(()).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), last_rx)
            .await
            .unwrap()
            .unwrap()
    );
    let result = tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(error) if error.kind() == &Error::BodyTooLarge));
}

#[tokio::test]
async fn final_frame_after_deadline_is_refused() {
    let (first_tx, first_rx) = tokio::sync::oneshot::channel();
    let (go_tx, go_rx) = tokio::sync::oneshot::channel();
    let (last_tx, last_rx) = tokio::sync::oneshot::channel();
    let (head_tx, head_rx) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::Fragment {
        first: first_tx,
        proceed: go_rx,
        last: last_tx,
    }]);
    let client = client(&server);
    let base = TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
    let deadline = base.monotonic_now() + Duration::from_secs(1);
    let clock = SignalingClock {
        inner: base.clone(),
        sleeps: Arc::new(AtomicUsize::new(0)),
        head: Arc::new(Mutex::new(Some(head_tx))),
        silent: true,
    };
    let request = req(Method::GET, &server.origin);
    let worker = tokio::spawn(async move {
        send::once(&client, request, 10, Encoding::Identity, &clock, deadline).await
    });
    tokio::time::timeout(Duration::from_secs(3), first_rx)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), head_rx)
        .await
        .unwrap()
        .unwrap();
    base.advance(Duration::from_secs(1));
    go_tx.send(()).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), last_rx)
            .await
            .unwrap()
            .unwrap()
    );
    let result = tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(result, Err(error) if error.kind() == &Error::Deadline && error.observed().is_some())
    );
}

#[tokio::test]
async fn frame_deadline_precedes_overflow() {
    let server = Server::start(vec![Response::Raw(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nabcd\r\n0\r\n\r\n".to_vec(),
    )]);
    let client = client(&server);
    let start = tokio::time::Instant::now();
    let clock = BoundaryClock {
        start,
        sleeps: AtomicUsize::new(0),
        checks: AtomicUsize::new(0),
        threshold: 2,
    };
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        send::once(
            &client,
            req(Method::GET, &server.origin),
            3,
            Encoding::Identity,
            &clock,
            start + Duration::from_secs(1),
        ),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(error) if error.kind() == &Error::Deadline));
}

#[tokio::test]
async fn final_deadline_check_rejects_completed_response() {
    let server = Server::start(vec![response("200 OK", &[("Content-Length", "1")], b"x")]);
    let client = client(&server);
    let start = tokio::time::Instant::now();
    let clock = BoundaryClock {
        start,
        sleeps: AtomicUsize::new(0),
        checks: AtomicUsize::new(0),
        threshold: 5,
    };
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        send::once(
            &client,
            req(Method::GET, &server.origin),
            10,
            Encoding::Identity,
            &clock,
            start + Duration::from_secs(1),
        ),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(error) if error.kind() == &Error::Deadline));
}

#[tokio::test]
async fn request_guards_refuse_connect_and_upgrade_before_connecting() {
    let server = Server::start(vec![response("200 OK", &[("Content-Length", "0")], b"")]);
    let client = client(&server);
    assert!(matches!(
        send(&client, req(Method::CONNECT, &server.origin), 10).await,
        Err(Error::InvalidRequest)
    ));
    let mut upgrade = req(Method::GET, &server.origin);
    upgrade
        .headers_mut()
        .insert(http::header::UPGRADE, "websocket".parse().unwrap());
    assert!(matches!(
        send(&client, upgrade, 10).await,
        Err(Error::InvalidRequest)
    ));
    let mut connection = req(Method::GET, &server.origin);
    connection.headers_mut().insert(
        http::header::CONNECTION,
        "keep-alive, Upgrade".parse().unwrap(),
    );
    assert!(matches!(
        send(&client, connection, 10).await,
        Err(Error::InvalidRequest)
    ));
    assert_eq!(server.connections(), 0);
    assert!(
        send(&client, req(Method::GET, &server.origin), 10)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn invalid_uri_is_refused_before_connecting() {
    let server = Server::start(vec![response("200 OK", &[("Content-Length", "0")], b"")]);
    let client = client(&server);
    assert!(matches!(
        send(&client, req(Method::GET, "/private?token=toy"), 10).await,
        Err(Error::InvalidRequest)
    ));
    assert!(matches!(
        send(
            &client,
            req(Method::GET, &server.origin.replacen("https:", "http:", 1)),
            10
        )
        .await,
        Err(Error::InvalidRequest)
    ));
    assert_eq!(server.connections(), 0);
    assert!(
        send(&client, req(Method::GET, &server.origin), 10)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn switching_protocols_is_refused() {
    let server = Server::start(vec![
        response(
            "101 Switching Protocols",
            &[("Connection", "upgrade"), ("Upgrade", "websocket")],
            b"",
        ),
        response("200 OK", &[("Content-Length", "0")], b""),
    ]);
    let client = client(&server);
    assert!(matches!(
        send(&client, req(Method::GET, &server.origin), 10).await,
        Err(error) if error.kind() == &Error::SwitchingProtocols
    ));
    assert!(
        send(&client, req(Method::GET, &server.origin), 10)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn nonidentity_encoding_is_refused() {
    let server = Server::start(vec![
        response(
            "200 OK",
            &[("Content-Encoding", "gzip"), ("Content-Length", "0")],
            b"",
        ),
        response(
            "200 OK",
            &[("Content-Encoding", "identity"), ("Content-Length", "0")],
            b"",
        ),
    ]);
    let client = client(&server);
    assert!(matches!(
        send(&client, req(Method::GET, &server.origin), 10).await,
        Err(error) if error.kind() == &Error::Encoding
    ));
    assert!(
        send(&client, req(Method::GET, &server.origin), 10)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn mixed_identity_and_gzip_encoding_is_refused() {
    let server = Server::start(vec![response(
        "200 OK",
        &[
            ("Content-Encoding", "identity, gzip"),
            ("Content-Length", "0"),
        ],
        b"",
    )]);
    let client = client(&server);
    assert!(
        matches!(send(&client, req(Method::GET, &server.origin), 10).await, Err(error) if error.kind() == &Error::Encoding)
    );
}

#[tokio::test]
async fn early_refusal_does_not_reuse_pooled_connection() {
    let server = Server::start(vec![
        response(
            "200 OK",
            &[("Content-Encoding", "gzip"), ("Content-Length", "0")],
            b"",
        ),
        response("200 OK", &[("Content-Length", "0")], b""),
    ]);
    let client =
        connection::client(true, HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap();
    assert!(
        matches!(send(&client, req(Method::GET, &server.origin), 10).await, Err(error) if error.kind() == &Error::Encoding)
    );
    assert!(
        send(&client, req(Method::GET, &server.origin), 10)
            .await
            .is_ok()
    );
    assert_eq!(server.connections(), 2);
}

#[tokio::test]
async fn redirect_is_returned_without_following() {
    let server = Server::start(vec![
        response(
            "302 Found",
            &[("Location", "/next"), ("Content-Length", "0")],
            b"",
        ),
        response("200 OK", &[("Content-Length", "0")], b""),
    ]);
    let client = client(&server);
    let reply = send(&client, req(Method::GET, &server.origin), 10)
        .await
        .unwrap();
    assert_eq!(reply.status, http::StatusCode::FOUND);
    assert_eq!(reply.headers[http::header::LOCATION], "/next");
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn declared_oversize_is_refused_before_body() {
    let (started, wait) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::Stall(started, 11)]);
    let first_client = client(&server);
    let uri = server.origin.clone();
    let worker = tokio::spawn(async move { send(&first_client, req(Method::GET, &uri), 10).await });
    tokio::time::timeout(Duration::from_secs(3), wait)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), worker).await,
        Ok(Ok(Err(error))) if error.kind() == &Error::BodyTooLarge
    ));
    let server = Server::start(vec![
        response("200 OK", &[("Content-Length", "999")], b""),
        response("304 Not Modified", &[("Content-Length", "999")], b""),
        response("204 No Content", &[("Content-Length", "999")], b""),
    ]);
    let client = client(&server);
    assert!(
        send(&client, req(Method::HEAD, &server.origin), 10)
            .await
            .is_ok()
    );
    assert!(
        send(&client, req(Method::GET, &server.origin), 10)
            .await
            .is_ok()
    );
    assert!(
        send(&client, req(Method::GET, &server.origin), 10)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn oversized_body_error_retains_observed_head() {
    let server = Server::start(vec![response(
        "401 Unauthorized",
        &[("Content-Length", "11"), ("X-Policy", "value")],
        b"01234567890",
    )]);
    let client = client(&server);
    let error = match send(&client, req(Method::GET, &server.origin), 10).await {
        Err(e) => e,
        Ok(_) => panic!("expected error"),
    };
    assert!(!format!("{error:?}").contains("value"));
    let head = error.observed();
    assert!(head.is_some());
    let head = head.unwrap();
    assert_eq!(head.status, http::StatusCode::UNAUTHORIZED);
    assert_eq!(head.headers["X-Policy"], "value");
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn truncated_body_error_retains_observed_head() {
    let server = Server::start(vec![Response::RawClose(
        b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 3\r\nX-Policy: value\r\n\r\na".to_vec(),
    )]);
    let client = client(&server);
    let error = match send(&client, req(Method::GET, &server.origin), 10).await {
        Err(e) => e,
        Ok(_) => panic!("expected body error"),
    };
    assert_eq!(error.kind(), &Error::Body);
    assert!(error.observed().is_some());
    let head = error.observed().unwrap();
    assert_eq!(head.status, http::StatusCode::UNAUTHORIZED);
    assert_eq!(head.headers["X-Policy"], "value");
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn repeated_content_length_is_bounded_before_body() {
    let server = Server::start(vec![Response::Raw(
        b"HTTP/1.1 200 OK\r\nContent-Length: 11, 11\r\n\r\n".to_vec(),
    )]);
    let client = client(&server);
    assert!(
        matches!(tokio::time::timeout(Duration::from_secs(1), send(&client, req(Method::GET, &server.origin), 10)).await, Ok(Err(error)) if error.kind() == &Error::BodyTooLarge)
    );
}

#[tokio::test]
async fn deadline_covers_stalled_response_body() {
    let (started, wait) = tokio::sync::oneshot::channel();
    let (eof_tx, eof_rx) = tokio::sync::oneshot::channel();
    let (head_tx, head_rx) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::ObserveStall {
        head: b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 1\r\nX-Policy: value\r\n\r\n".to_vec(),
        sent: started,
        eof: eof_tx,
    }]);
    let client = client(&server);
    let base = TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
    let deadline = base.monotonic_now() + Duration::from_secs(1);
    let clock = SignalingClock {
        inner: base.clone(),
        sleeps: Arc::new(AtomicUsize::new(0)),
        head: Arc::new(Mutex::new(Some(head_tx))),
        silent: false,
    };
    let request = req(Method::GET, &server.origin);
    let worker = tokio::spawn(async move {
        send::once(&client, request, 10, Encoding::Identity, &clock, deadline).await
    });
    tokio::time::timeout(Duration::from_secs(3), wait)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), head_rx)
        .await
        .unwrap()
        .unwrap();
    base.advance(Duration::from_secs(1));
    let result = tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("expected deadline"),
    };
    assert_eq!(error.kind(), &Error::Deadline);
    assert!(error.observed().is_some());
    let head = error.observed().unwrap();
    assert_eq!(head.status, http::StatusCode::UNAUTHORIZED);
    assert_eq!(head.headers["X-Policy"], "value");
    assert_eq!(server.requests(), 1);
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), eof_rx).await,
        Ok(Ok(true))
    ));
}
