use bytes::Bytes;
use http_body_util::Full;
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::{
    client::legacy::{Client as HyperClient, connect::HttpConnector},
    rt::{TokioExecutor, TokioTimer},
};
use rustls::{ClientConfig, RootCertStore};
use std::{sync::Arc, time::Duration};

pub(crate) type Client = HyperClient<HttpsConnector<HttpConnector>, Full<Bytes>>;

pub(crate) fn client(pooled: bool) -> Result<Client, rustls::Error> {
    let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    build(pooled, roots)
}

#[cfg(test)]
pub(crate) fn test_client(
    pooled: bool,
    root: rustls::pki_types::CertificateDer<'static>,
) -> Client {
    let mut roots = RootCertStore::empty();
    roots.add(root).unwrap();
    build(pooled, roots).unwrap()
}

fn build(pooled: bool, roots: RootCertStore) -> Result<Client, rustls::Error> {
    let mut tls =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth();
    tls.enable_early_data = false;
    let mut http = HttpConnector::new();
    http.enforce_http(false);
    let connector = HttpsConnectorBuilder::new()
        .with_tls_config(tls)
        .https_only()
        .enable_http1()
        .wrap_connector(http);
    let mut builder = HyperClient::builder(TokioExecutor::new());
    builder
        .retry_canceled_requests(false)
        .pool_timer(TokioTimer::new())
        .pool_idle_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(if pooled { 2 } else { 0 });
    Ok(builder.build(connector))
}
