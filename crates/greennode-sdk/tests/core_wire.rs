#[allow(dead_code)]
#[path = "../src/wire/connection.rs"]
mod connection;
mod support;

use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::{
    ClientConfig, RootCertStore,
    pki_types::{CertificateDer, pem::PemObject},
};

#[tokio::test]
async fn scripted_https_response_is_served_and_counted() {
    let server = support::https::Server::start(vec![
        support::https::Response::Http {
            status: "200 OK".into(),
            headers: vec![("Content-Length".into(), "2".into())],
            body: b"ok".to_vec(),
        },
        support::https::Response::Raw(Vec::new()),
    ]);
    assert_eq!(
        server.origin,
        format!("https://localhost:{}", server.address.port())
    );
    let mut roots = RootCertStore::empty();
    let root = CertificateDer::pem_slice_iter(server.root_pem.as_bytes())
        .next()
        .unwrap()
        .unwrap();
    roots.add(root).unwrap();
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
    let mut bytes = vec![0; b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".len()];
    tls.read_exact(&mut bytes).await.unwrap();
    assert_eq!(bytes, b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
    assert_eq!(server.connections(), 1);
    assert_eq!(server.requests(), 1);
}

fn response() -> support::https::Response {
    support::https::Response::Http {
        status: "200 OK".into(),
        headers: vec![("Content-Length".into(), "0".into())],
        body: Vec::new(),
    }
}

fn root(server: &support::https::Server) -> CertificateDer<'static> {
    CertificateDer::pem_slice_iter(server.root_pem.as_bytes())
        .next()
        .unwrap()
        .unwrap()
}

async fn request(
    client: &connection::Client,
    uri: String,
) -> Result<(), hyper_util::client::legacy::Error> {
    use http_body_util::{BodyExt, Full};
    let req = http::Request::get(uri)
        .body(Full::new(bytes::Bytes::new()))
        .unwrap();
    let response = client.request(req).await?;
    response.into_body().collect().await.unwrap();
    Ok(())
}

#[tokio::test]
async fn client_refuses_http_origin() {
    let server = support::https::Server::start(vec![response()]);
    let client = connection::test_client(true, root(&server));
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
    let server = support::https::Server::start(vec![response(), response()]);
    let client = connection::test_client(true, root(&server));
    request(&client, server.origin.clone()).await.unwrap();
    request(&client, server.origin.clone()).await.unwrap();
    assert_eq!(server.requests(), 2);
    assert_eq!(server.connections(), 1);
}

#[tokio::test]
async fn one_send_client_does_not_reuse_connection() {
    let server = support::https::Server::start(vec![response(), response()]);
    let client = connection::test_client(false, root(&server));
    request(&client, server.origin.clone()).await.unwrap();
    request(&client, server.origin.clone()).await.unwrap();
    assert_eq!(server.requests(), 2);
    assert_eq!(server.connections(), 2);
}

#[tokio::test]
async fn client_refuses_untrusted_certificate() {
    let trusted = support::https::Server::start(vec![response()]);
    let untrusted = support::https::Server::start(vec![response()]);
    let client = connection::test_client(true, root(&trusted));
    assert!(request(&client, untrusted.origin.clone()).await.is_err());
    assert_eq!(untrusted.requests(), 0);
    assert!(request(&client, trusted.origin.clone()).await.is_ok());
}
