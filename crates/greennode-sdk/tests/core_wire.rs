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
