use super::{
    connection::{self, HeadLimit, Trust},
    send::{self, Encoding, Error},
    test_server::{Response, Server},
};
use crate::{Clock, TestClock};
use bytes::Bytes;
use http::{Method, Request};
use http_body_util::Full;
use std::time::Duration;

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
        Err(Error::BodyTooLarge)
    ));
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
        Err(Error::SwitchingProtocols)
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
        Err(Error::Encoding)
    ));
    assert!(
        send(&client, req(Method::GET, &server.origin), 10)
            .await
            .is_ok()
    );
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
        Ok(Ok(Err(Error::BodyTooLarge)))
    ));
    let server = Server::start(vec![
        response("200 OK", &[("Content-Length", "999")], b""),
        response("304 Not Modified", &[("Content-Length", "999")], b""),
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
}

#[tokio::test]
async fn deadline_covers_stalled_response_body() {
    let (started, wait) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::Stall(started, 1)]);
    let client = client(&server);
    let clock = TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
    let deadline = clock.monotonic_now() + Duration::from_secs(1);
    let request = req(Method::GET, &server.origin);
    let worker_clock = clock.clone();
    let worker = tokio::spawn(async move {
        send::once(
            &client,
            request,
            10,
            Encoding::Identity,
            &worker_clock,
            deadline,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(3), wait)
        .await
        .unwrap()
        .unwrap();
    clock.advance(Duration::from_secs(1));
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), worker).await,
        Ok(Ok(Err(Error::Deadline)))
    ));
    assert_eq!(server.requests(), 1);
}
