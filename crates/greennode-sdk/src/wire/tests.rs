use super::{connection, test_server};

use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::{ClientConfig, RootCertStore, pki_types::CertificateDer};

#[tokio::test]
async fn scripted_https_response_is_served_and_counted() {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let server = test_server::Server::start(vec![
            test_server::Response::Http {
                status: "200 OK".into(),
                headers: vec![("Content-Length".into(), "2".into())],
                body: b"ok".to_vec(),
            },
            test_server::Response::Raw(Vec::new()),
        ]);
        assert_eq!(
            server.origin,
            format!("https://localhost:{}", server.address.port())
        );
        let mut roots = RootCertStore::empty();
        roots.add(server.root_der.clone()).unwrap();
        let config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let tcp = TcpStream::connect(server.address).await.unwrap();
        let mut tls = TlsConnector::from(Arc::new(config))
            .connect("localhost".try_into().unwrap(), tcp)
            .await
            .unwrap();
        tls.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        tls.flush().await.unwrap();
        let mut bytes = vec![0; b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".len()];
        tls.read_exact(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        assert_eq!(server.connections(), 1);
        assert_eq!(server.requests(), 1);
    })
    .await
    .unwrap();
}

fn response() -> test_server::Response {
    test_server::Response::Http {
        status: "200 OK".into(),
        headers: vec![("Content-Length".into(), "0".into())],
        body: Vec::new(),
    }
}

fn root(server: &test_server::Server) -> CertificateDer<'static> {
    server.root_der.clone()
}

fn test_client(
    pooled: bool,
    limit: connection::HeadLimit,
    root: CertificateDer<'static>,
) -> connection::Client {
    connection::client(pooled, limit, connection::Trust::test(root)).unwrap()
}

async fn request(
    client: &connection::Client,
    uri: String,
) -> Result<(), hyper_util::client::legacy::Error> {
    use http_body_util::{BodyExt, Full};
    let req = http::Request::get(uri)
        .body(Full::new(bytes::Bytes::new()))
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let response = client.request(req).await?;
        response.into_body().collect().await.unwrap();
        Ok(())
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn client_refuses_http_origin() {
    let server = test_server::Server::start(vec![response()]);
    let client = test_client(true, connection::HeadLimit::Api, root(&server));
    assert!(
        request(&client, server.origin.replacen("https:", "http:", 1))
            .await
            .is_err()
    );
    assert_eq!(server.connections(), 0);
    assert!(request(&client, server.origin.clone()).await.is_ok());
}

#[tokio::test]
async fn pooled_client_reuses_connection() {
    let server = test_server::Server::start(vec![response(), response()]);
    let client = test_client(true, connection::HeadLimit::Api, root(&server));
    request(&client, server.origin.clone()).await.unwrap();
    request(&client, server.origin.clone()).await.unwrap();
    assert_eq!(server.requests(), 2);
    assert_eq!(server.connections(), 1);
}

#[tokio::test]
async fn one_send_client_does_not_reuse_connection() {
    let server = test_server::Server::start(vec![response(), response()]);
    let client = test_client(false, connection::HeadLimit::Api, root(&server));
    request(&client, server.origin.clone()).await.unwrap();
    request(&client, server.origin.clone()).await.unwrap();
    assert_eq!(server.requests(), 2);
    assert_eq!(server.connections(), 2);
}

#[tokio::test]
async fn client_refuses_oversized_response_head() {
    let server = test_server::Server::start(vec![
        test_server::Response::Http {
            status: "200 OK".into(),
            headers: vec![
                ("X-Large".into(), "a".repeat(128 * 1024)),
                ("Content-Length".into(), "0".into()),
            ],
            body: Vec::new(),
        },
        response(),
    ]);
    let client = test_client(true, connection::HeadLimit::Api, root(&server));
    assert!(request(&client, server.origin.clone()).await.is_err());
    assert!(request(&client, server.origin.clone()).await.is_ok());
}

#[tokio::test]
async fn s3_head_limit_is_smaller_than_api_head_limit() {
    let large = || test_server::Response::Http {
        status: "200 OK".into(),
        headers: vec![
            ("X-Large".into(), "a".repeat(48 * 1024)),
            ("Content-Length".into(), "0".into()),
        ],
        body: Vec::new(),
    };
    let s3 = test_server::Server::start(vec![large()]);
    let client = test_client(false, connection::HeadLimit::S3, root(&s3));
    assert!(request(&client, s3.origin.clone()).await.is_err());
    let api = test_server::Server::start(vec![large()]);
    let client = test_client(false, connection::HeadLimit::Api, root(&api));
    assert!(request(&client, api.origin.clone()).await.is_ok());
}

#[tokio::test]
async fn server_offering_h2_does_not_negotiate_it() {
    let server = test_server::Server::start(vec![response()]);
    let client = test_client(true, connection::HeadLimit::Api, root(&server));
    let uri = server.origin.clone();
    let task = tokio::spawn(async move { request(&client, uri).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while server.protocol() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_ne!(server.protocol(), 3);
    assert!(task.await.unwrap().is_ok());
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn pooled_connection_closed_after_request_is_not_replayed() {
    let server =
        test_server::Server::start(vec![response(), test_server::Response::Close, response()]);
    let client = test_client(true, connection::HeadLimit::Api, root(&server));
    request(&client, server.origin.clone()).await.unwrap();
    let before = server.requests();
    assert!(request(&client, server.origin.clone()).await.is_err());
    assert_eq!(server.requests() - before, 1);
    assert_eq!(server.connections(), 1);
}

#[tokio::test]
async fn client_refuses_untrusted_certificate() {
    let trusted = test_server::Server::start(vec![response()]);
    let untrusted = test_server::Server::start(vec![response()]);
    let client = test_client(true, connection::HeadLimit::Api, root(&trusted));
    assert!(request(&client, untrusted.origin.clone()).await.is_err());
    assert_eq!(untrusted.requests(), 0);
    assert!(request(&client, trusted.origin.clone()).await.is_ok());
}
