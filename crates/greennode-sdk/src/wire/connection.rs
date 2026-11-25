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

#[derive(Clone, Copy)]
pub(crate) enum HeadLimit {
    Api,
    S3,
}

pub(crate) struct Trust(RootCertStore);

impl Trust {
    pub(crate) fn webpki() -> Self {
        Self(RootCertStore::from_iter(
            webpki_roots::TLS_SERVER_ROOTS.iter().cloned(),
        ))
    }

    #[cfg(test)]
    pub(crate) fn test(root: rustls::pki_types::CertificateDer<'static>) -> Self {
        let mut roots = RootCertStore::empty();
        roots.add(root).unwrap();
        Self(roots)
    }
}

pub(crate) fn client(
    pooled: bool,
    limit: HeadLimit,
    trust: Trust,
) -> Result<Client, rustls::Error> {
    let mut tls =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(trust.0)
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
        .http1_max_buf_size(match limit {
            HeadLimit::Api => 64 * 1024,
            HeadLimit::S3 => 32 * 1024,
        })
        .pool_timer(TokioTimer::new())
        .pool_idle_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(if pooled { 2 } else { 0 });
    Ok(builder.build(connector))
}
