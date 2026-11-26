use bytes::Bytes;
use http_body_util::Full;
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::{
    client::legacy::{Client as HyperClient, connect::HttpConnector},
    rt::{TokioExecutor, TokioTimer},
};
use rustls::{ClientConfig, RootCertStore};
use std::{
    error::Error,
    fmt,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tower_service::Service;

#[derive(Clone)]
pub(crate) struct DialConnector(HttpConnector);

pub(crate) struct DialFailure(Box<dyn Error + Send + Sync>);

impl fmt::Debug for DialFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DialFailure")
    }
}
impl fmt::Display for DialFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("dial failed")
    }
}
impl Error for DialFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}
impl Service<http::Uri> for DialConnector {
    type Response = <HttpConnector as Service<http::Uri>>::Response;
    type Error = DialFailure;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx).map_err(|e| DialFailure(Box::new(e)))
    }

    fn call(&mut self, uri: http::Uri) -> Self::Future {
        let future = self.0.call(uri);
        Box::pin(async move { future.await.map_err(|e| DialFailure(Box::new(e))) })
    }
}

pub(crate) type Client = HyperClient<HttpsConnector<DialConnector>, Full<Bytes>>;

pub(crate) struct PooledClient(Client);
pub(crate) struct OneSendClient(Client);

impl PooledClient {
    pub(crate) fn new(limit: HeadLimit, trust: Trust) -> Result<Self, rustls::Error> {
        client(true, limit, trust).map(Self)
    }
    pub(crate) fn as_client(&self) -> &Client {
        &self.0
    }
}
impl OneSendClient {
    pub(crate) fn new(limit: HeadLimit, trust: Trust) -> Result<Self, rustls::Error> {
        client(false, limit, trust).map(Self)
    }
    pub(crate) fn as_client(&self) -> &Client {
        &self.0
    }
}

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
    pub(crate) fn test_roots(
        roots: impl IntoIterator<Item = rustls::pki_types::CertificateDer<'static>>,
    ) -> Self {
        let mut store = RootCertStore::empty();
        for root in roots {
            store.add(root).unwrap();
        }
        Self(store)
    }

    #[cfg(any(test, feature = "test-support"))]
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
        .wrap_connector(DialConnector(http));
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

#[cfg(test)]
mod tests {
    use super::Trust;

    #[test]
    fn webpki_trust_uses_bundled_roots() {
        let roots = Trust::webpki().0;
        assert!(!roots.is_empty());
        assert_eq!(roots.len(), webpki_roots::TLS_SERVER_ROOTS.len());
    }
}
