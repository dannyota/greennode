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
pub(crate) struct DialConnector(
    HttpConnector,
    #[cfg(feature = "test-support")] Option<std::net::SocketAddr>,
);

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
        #[cfg(feature = "test-support")]
        let uri = if let Some(address) = self.1 {
            if !uri
                .host()
                .is_some_and(|host| fixture_host(address, host, uri.port_u16().unwrap_or(443)))
            {
                return Box::pin(async {
                    Err(DialFailure(Box::new(std::io::Error::other(
                        "endpoint outside the test fixture",
                    ))))
                });
            }
            match format!("https://{address}").parse() {
                Ok(pinned) => pinned,
                Err(error) => return Box::pin(async move { Err(DialFailure(Box::new(error))) }),
            }
        } else {
            uri
        };
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

pub(crate) struct Trust(
    RootCertStore,
    #[cfg(feature = "test-support")] Option<std::net::SocketAddr>,
);

#[cfg(feature = "test-support")]
fn fixture_host(address: std::net::SocketAddr, host: &str, port: u16) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    address.ip().is_loopback()
        && port == address.port()
        && (host.eq_ignore_ascii_case("localhost") || host.parse() == Ok(address.ip()))
}

#[cfg(feature = "test-support")]
#[derive(Clone)]
pub(crate) struct FixtureTrust {
    address: std::net::SocketAddr,
    root: rustls::pki_types::CertificateDer<'static>,
}

#[cfg(feature = "test-support")]
impl FixtureTrust {
    pub(crate) fn new(server: &crate::wire::test_server::Server) -> Self {
        let (address, root) = server.fixture();
        Self {
            address,
            root: root.clone(),
        }
    }

    pub(crate) fn allows(&self, endpoint: &str) -> bool {
        url::Url::parse(endpoint).is_ok_and(|url| {
            url.scheme() == "https"
                && url
                    .host_str()
                    .is_some_and(|host| fixture_host(self.address, host, url.port().unwrap_or(443)))
        })
    }

    pub(crate) fn trust(&self) -> Result<Trust, rustls::Error> {
        let mut roots = RootCertStore::empty();
        roots.add(self.root.clone())?;
        Ok(Trust(roots, Some(self.address)))
    }
}

impl Trust {
    fn roots(roots: RootCertStore) -> Self {
        Self(
            roots,
            #[cfg(feature = "test-support")]
            None,
        )
    }

    pub(crate) fn webpki() -> Self {
        Self::roots(RootCertStore::from_iter(
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
        Self::roots(store)
    }

    #[cfg(test)]
    pub(crate) fn test(root: rustls::pki_types::CertificateDer<'static>) -> Self {
        let mut roots = RootCertStore::empty();
        roots.add(root).unwrap();
        Self::roots(roots)
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
        .wrap_connector(DialConnector(
            http,
            #[cfg(feature = "test-support")]
            trust.1,
        ));
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
