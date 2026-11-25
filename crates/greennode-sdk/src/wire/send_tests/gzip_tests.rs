use super::*;

async fn gzip_send(
    body: &[u8],
    encoding: &str,
    cap: usize,
    policy: Encoding,
) -> Result<send::Reply, Error> {
    let length = body.len().to_string();
    let server = Server::start(vec![response(
        "401 Unauthorized",
        &[("Content-Encoding", encoding), ("Content-Length", &length)],
        body,
    )]);
    let client = client(&server);
    let clock = crate::SystemClock;
    send::once(
        &client,
        req(Method::GET, &server.origin),
        cap,
        policy,
        &clock,
        clock.monotonic_now() + Duration::from_secs(5),
    )
    .await
}

const GZIP_ABC: &[u8] = &[
    31, 139, 8, 0, 0, 0, 0, 0, 2, 3, 75, 76, 74, 6, 0, 194, 65, 36, 53, 3, 0, 0, 0,
];

#[tokio::test]
async fn gzip_deadline_precedes_decoded_overflow() {
    let server = Server::start(vec![response(
        "401 Unauthorized",
        &[("Content-Encoding", "gzip"), ("Content-Length", "23")],
        GZIP_ABC,
    )]);
    let client = client(&server);
    let start = tokio::time::Instant::now();
    let clock = BoundaryClock {
        start,
        sleeps: AtomicUsize::new(0),
        checks: AtomicUsize::new(0),
        threshold: 6,
    };
    let result = send::once(
        &client,
        req(Method::GET, &server.origin),
        2,
        Encoding::Gzip,
        &clock,
        start + Duration::from_secs(1),
    )
    .await;
    assert!(matches!(result, Err(e) if e.kind() == &Error::Deadline && e.observed().is_some()));
}

#[tokio::test]
async fn identity_lists_are_refused_for_both_policies() {
    for policy in [Encoding::Identity, Encoding::Gzip] {
        assert!(gzip_send(b"abc", "identity", 3, policy).await.is_ok());
        assert!(
            matches!(gzip_send(b"abc", "identity, identity", 3, policy).await, Err(e) if e.kind() == &Error::Encoding)
        );
        let server = Server::start(vec![response(
            "200 OK",
            &[
                ("Content-Encoding", "identity"),
                ("Content-Encoding", "identity"),
                ("Content-Length", "3"),
            ],
            b"abc",
        )]);
        let client = client(&server);
        let clock = crate::SystemClock;
        let result = send::once(
            &client,
            req(Method::GET, &server.origin),
            3,
            policy,
            &clock,
            clock.monotonic_now() + Duration::from_secs(5),
        )
        .await;
        assert!(matches!(result, Err(e) if e.kind() == &Error::Encoding));
    }
}

#[tokio::test]
async fn gzip_decoded_cap_rejects_overflow() {
    assert!(gzip_send(GZIP_ABC, "gzip", 3, Encoding::Gzip).await.is_ok());
    assert!(
        matches!(gzip_send(GZIP_ABC, "gzip", 2, Encoding::Gzip).await, Err(e) if e.kind() == &Error::BodyTooLarge && e.observed().is_some())
    );
}

#[tokio::test]
async fn gzip_encoded_cap_rejects_overflow() {
    assert!(gzip_send(GZIP_ABC, "gzip", 3, Encoding::Gzip).await.is_ok());
    let (sent, wait) = tokio::sync::oneshot::channel();
    let (eof, _eof_rx) = tokio::sync::oneshot::channel();
    let head =
        b"HTTP/1.1 401 Unauthorized\r\nContent-Encoding: gzip\r\nContent-Length: 65540\r\n\r\n"
            .to_vec();
    let server = Server::start(vec![Response::ObserveStall { head, sent, eof }]);
    let client = client(&server);
    let clock = crate::SystemClock;
    let request = req(Method::GET, &server.origin);
    let worker = tokio::spawn(async move {
        send::once(
            &client,
            request,
            3,
            Encoding::Gzip,
            &clock,
            clock.monotonic_now() + Duration::from_secs(5),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(3), wait)
        .await
        .unwrap()
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(1), worker).await;
    assert!(
        matches!(result, Ok(Ok(Err(e))) if e.kind() == &Error::BodyTooLarge && e.observed().is_some())
    );
}

#[tokio::test]
async fn gzip_streamed_encoded_cap_rejects_overflow() {
    assert!(gzip_send(GZIP_ABC, "gzip", 3, Encoding::Gzip).await.is_ok());
    for extra in [65514usize, 65515] {
        let mut body = GZIP_ABC[..10].to_vec();
        body[3] = 4;
        body.extend_from_slice(&(extra as u16).to_le_bytes());
        body.extend_from_slice(&vec![0; extra]);
        body.extend_from_slice(&GZIP_ABC[10..]);
        let mut raw = format!("HTTP/1.1 401 Unauthorized\r\nContent-Encoding: gzip\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n", body.len()).into_bytes();
        raw.extend_from_slice(&body);
        raw.extend_from_slice(b"\r\n0\r\n\r\n");
        let server = Server::start(vec![Response::Raw(raw)]);
        let client = client(&server);
        let clock = crate::SystemClock;
        let result = send::once(
            &client,
            req(Method::GET, &server.origin),
            3,
            Encoding::Gzip,
            &clock,
            clock.monotonic_now() + Duration::from_secs(5),
        )
        .await;
        if extra == 65514 {
            assert!(matches!(result, Ok(reply) if reply.body == b"abc"));
        } else {
            assert!(
                matches!(result, Err(e) if e.kind() == &Error::BodyTooLarge && e.observed().is_some())
            );
        }
    }
}

#[tokio::test]
async fn gzip_multiple_members_are_refused() {
    assert!(gzip_send(GZIP_ABC, "gzip", 6, Encoding::Gzip).await.is_ok());
    let mut body = GZIP_ABC.to_vec();
    body.extend_from_slice(GZIP_ABC);
    assert!(
        matches!(gzip_send(&body, "gzip", 6, Encoding::Gzip).await, Err(e) if e.kind() == &Error::Body && e.observed().is_some())
    );
}

#[tokio::test]
async fn gzip_stalled_body_obeys_deadline() {
    assert!(gzip_send(GZIP_ABC, "gzip", 3, Encoding::Gzip).await.is_ok());
    let (sent, wait) = tokio::sync::oneshot::channel();
    let (eof, _eof_rx) = tokio::sync::oneshot::channel();
    let (head_tx, head_rx) = tokio::sync::oneshot::channel();
    let mut head =
        b"HTTP/1.1 401 Unauthorized\r\nContent-Encoding: gzip\r\nContent-Length: 23\r\n\r\n"
            .to_vec();
    head.extend_from_slice(&GZIP_ABC[..12]);
    let server = Server::start(vec![Response::ObserveStall { head, sent, eof }]);
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
        send::once(&client, request, 3, Encoding::Gzip, &clock, deadline).await
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
    let result = tokio::time::timeout(Duration::from_secs(1), worker).await;
    assert!(
        matches!(result, Ok(Ok(Err(e))) if e.kind() == &Error::Deadline && e.observed().is_some())
    );
}

#[tokio::test]
async fn gzip_truncation_is_refused() {
    assert!(gzip_send(GZIP_ABC, "gzip", 3, Encoding::Gzip).await.is_ok());
    assert!(
        matches!(gzip_send(&GZIP_ABC[..22], "gzip", 3, Encoding::Gzip).await, Err(e) if e.kind() == &Error::Body && e.observed().is_some())
    );
}

#[tokio::test]
async fn gzip_trailing_bytes_are_refused() {
    assert!(gzip_send(GZIP_ABC, "gzip", 3, Encoding::Gzip).await.is_ok());
    let mut body = GZIP_ABC.to_vec();
    body.push(0);
    assert!(
        matches!(gzip_send(&body, "gzip", 3, Encoding::Gzip).await, Err(e) if e.kind() == &Error::Body && e.observed().is_some())
    );
}

#[tokio::test]
async fn gzip_encoding_list_is_refused() {
    assert!(gzip_send(GZIP_ABC, "gzip", 3, Encoding::Gzip).await.is_ok());
    assert!(
        matches!(gzip_send(GZIP_ABC, "gzip, gzip", 3, Encoding::Gzip).await, Err(e) if e.kind() == &Error::Encoding && e.observed().is_some())
    );
}

#[tokio::test]
async fn gzip_is_refused_for_identity_policy() {
    assert!(gzip_send(GZIP_ABC, "gzip", 3, Encoding::Gzip).await.is_ok());
    assert!(
        matches!(gzip_send(GZIP_ABC, "gzip", 3, Encoding::Identity).await, Err(e) if e.kind() == &Error::Encoding && e.observed().is_some())
    );
}

#[tokio::test]
async fn gzip_body_is_decoded() {
    let compressed = [
        31, 139, 8, 0, 0, 0, 0, 0, 2, 3, 75, 76, 74, 6, 0, 194, 65, 36, 53, 3, 0, 0, 0,
    ];
    let server = Server::start(vec![response(
        "200 OK",
        &[("Content-Encoding", "gzip"), ("Content-Length", "23")],
        &compressed,
    )]);
    let client = client(&server);
    let clock = crate::SystemClock;
    let reply = send::once(
        &client,
        req(Method::GET, &server.origin),
        3,
        Encoding::Gzip,
        &clock,
        clock.monotonic_now() + Duration::from_secs(5),
    )
    .await;
    assert!(matches!(reply, Ok(reply) if reply.body == b"abc"));
}
