#[cfg(test)]
mod tests;

use super::{EndpointOverrides, IamUserAuth, credentials};
use crate::discovery::{Discovery, ProjectRead};
use crate::endpoint::{self, Base, EndpointSet};
use crate::wire::connection::{HeadLimit, OneSendClient, PooledClient, Trust};
use crate::wire::product::sender::{BearerSource, Clients, Runtime, SentCredentials, Sleeper};
use crate::{Clock, Error, ErrorClass, Project, RequestContext, Secret};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::{path::PathBuf, time::Duration};

pub(crate) struct Session {
    auth: Auth,
    configured_credentials: SentCredentials,
    endpoints: EndpointSet,
    default_endpoints: EndpointSet,
    endpoint_overrides_used: bool,
    default_auth_endpoints: bool,
    #[cfg(any(test, feature = "test-support"))]
    product_fixtures: OnceLock<Vec<(crate::testing::Product, bool)>>,
    clients: OnceLock<Result<Clients, rustls::Error>>,
    clock: Arc<dyn Clock>,
    timeout: Duration,
    _profile: String,
    region: String,
    configured_project_id: Option<String>,
    discovery: Discovery,
    cdn_key: Option<Secret>,
}

#[allow(dead_code, reason = "Shared query for service ports")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthKind {
    ServiceAccount,
    IamUser,
    Static,
    Custom,
}

enum Auth {
    Provider(Arc<dyn crate::CredentialsProvider>),
    Static(Secret),
    ServiceAccount(Box<crate::session::service_account::ServiceAccount>),
    IamUser(Box<crate::session::iam_user::IamUser>),
}

impl BearerSource for Session {
    fn refreshable(&self) -> bool {
        !matches!(self.auth, Auth::Static(_))
    }

    fn token<'a>(
        &'a self,
        ctx: &'a RequestContext,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<crate::wire::product::sender::BearerToken, Error>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            match &self.auth {
                Auth::Static(token) => Ok(crate::wire::product::sender::BearerToken {
                    token: crate::Token::new(token.clone(), None),
                    obtained_at: None,
                    signin_credentials: None,
                }),
                Auth::Provider(provider) => {
                    crate::wire::product::sender::PublicSource(provider.as_ref())
                        .token(ctx)
                        .await
                }
                Auth::ServiceAccount(source) => source.token(ctx).await,
                Auth::IamUser(source) => source.token(ctx).await,
            }
        })
    }

    fn rejected<'a>(
        &'a self,
        ctx: &'a RequestContext,
        sent: &'a Secret,
        evict: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            match &self.auth {
                Auth::Static(_) => Ok(()),
                Auth::Provider(provider) => {
                    crate::wire::product::sender::PublicSource(provider.as_ref())
                        .rejected(ctx, sent, evict)
                        .await
                }
                Auth::ServiceAccount(source) => source.rejected(ctx, sent, evict).await,
                Auth::IamUser(source) => source.rejected(ctx, sent, evict).await,
            }
        })
    }
}

#[allow(dead_code, reason = "Shared queries for service ports")]
impl Session {
    pub(crate) fn auth_kind(&self) -> AuthKind {
        match self.auth {
            Auth::ServiceAccount(_) => AuthKind::ServiceAccount,
            Auth::IamUser(_) => AuthKind::IamUser,
            Auth::Static(_) => AuthKind::Static,
            Auth::Provider(_) => AuthKind::Custom,
        }
    }

    pub(crate) fn default_endpoints(&self) -> &EndpointSet {
        &self.default_endpoints
    }

    pub(crate) fn endpoint_overrides_used(&self) -> bool {
        self.endpoint_overrides_used
    }

    pub(crate) fn uses_default_auth_endpoints(&self) -> bool {
        self.default_auth_endpoints
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn is_write_fixture(&self, product: crate::testing::Product) -> bool {
        self.auth_kind() == AuthKind::Static
            && self
                .product_fixtures
                .get()
                .is_some_and(|fixtures| fixtures.contains(&(product, true)))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn has_product_fixture(&self, product: crate::testing::Product) -> bool {
        cfg!(debug_assertions)
            && self.auth_kind() == AuthKind::Static
            && self
                .product_fixtures
                .get()
                .is_some_and(|fixtures| fixtures.iter().any(|(fixture, _)| *fixture == product))
    }

    pub(crate) fn configured_project_id(&self) -> Option<&str> {
        self.configured_project_id.as_deref()
    }

    pub(crate) async fn project_id_context(
        &self,
        ctx: &RequestContext,
    ) -> Result<Option<String>, Box<Error>> {
        self.discovery.project_id_context(ctx, self.clock()).await
    }

    pub(crate) async fn require_project_id(
        &self,
        ctx: &RequestContext,
    ) -> Result<String, Box<Error>> {
        self.resolve_project(ctx, ProjectRead::Id)
            .await
            .map(|project| project.id)
    }

    pub(crate) async fn require_project_id_with_evidence(
        &self,
        ctx: &RequestContext,
        evidence: &mut crate::wire::product::evidence::Evidence,
    ) -> Result<String, Box<Error>> {
        let mut jitter = || self.retry_jitter();
        let runtime = self.runtime(ctx, &mut jitter)?;
        let base = self
            .endpoints()
            .vserver
            .as_ref()
            .ok_or_else(|| Box::new(Error::new(ErrorClass::InvalidConfig)))?;
        self.discovery
            .require_with_evidence(
                runtime,
                base,
                self.bearer_source(),
                &self.region,
                ProjectRead::Id,
                evidence,
            )
            .await
            .map(|project| project.id)
    }

    pub(crate) async fn require_project_id_with_limits(
        &self,
        ctx: &RequestContext,
        evidence: &mut crate::wire::product::evidence::Evidence,
        limits: &mut dyn crate::discovery::InventoryLimits,
    ) -> Result<String, Box<Error>> {
        let mut jitter = || self.retry_jitter();
        let runtime = self.runtime(ctx, &mut jitter)?;
        let base = self
            .endpoints()
            .vserver
            .as_ref()
            .ok_or_else(|| Box::new(Error::new(ErrorClass::InvalidConfig)))?;
        self.discovery
            .require_with_limits(
                runtime,
                base,
                self.bearer_source(),
                &self.region,
                ProjectRead::Id,
                evidence,
                Some(limits),
            )
            .await
            .map(|project| project.id)
    }

    pub(crate) async fn require_project(
        &self,
        ctx: &RequestContext,
    ) -> Result<Project, Box<Error>> {
        self.resolve_project(ctx, ProjectRead::Metadata).await
    }

    async fn resolve_project(
        &self,
        ctx: &RequestContext,
        read: ProjectRead,
    ) -> Result<Project, Box<Error>> {
        let mut jitter = || self.retry_jitter();
        let runtime = self.runtime(ctx, &mut jitter)?;
        let base = self
            .endpoints()
            .vserver
            .as_ref()
            .ok_or_else(|| Box::new(Error::new(ErrorClass::InvalidConfig)))?;
        self.discovery
            .require(runtime, base, self.bearer_source(), &self.region, read)
            .await
    }
}

impl Session {
    pub(crate) fn endpoints(&self) -> &EndpointSet {
        &self.endpoints
    }

    pub(crate) fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    pub(crate) fn cdn_key(&self) -> Option<&Secret> {
        self.cdn_key.as_ref()
    }

    pub(crate) fn bearer_source(&self) -> &dyn BearerSource {
        self
    }

    pub(crate) fn runtime<'a>(
        &'a self,
        ctx: &'a RequestContext,
        jitter: &'a mut (dyn FnMut() -> u64 + Send),
    ) -> Result<Runtime<'a>, Box<Error>> {
        if let Some(end) = ctx.check(self.clock.as_ref()) {
            return Err(Box::new(end.into()));
        }
        let deadline = self.clock.monotonic_now() + self.timeout;
        let client = self
            .clients
            .get_or_init(|| product_clients(|| Ok(Trust::webpki())))
            .as_ref()
            .map_err(|_| Box::new(Error::new(ErrorClass::InvalidConfig)))?;
        Ok(Runtime {
            client,
            ctx,
            configured_credentials: Some(&self.configured_credentials),
            clock: self.clock.as_ref(),
            deadline,
            sleeper: self,
            jitter,
        })
    }

    pub(crate) fn retry_jitter(&self) -> u64 {
        let mut bytes = [0; 8];
        match getrandom::fill(&mut bytes) {
            Ok(()) => u64::from_ne_bytes(bytes),
            Err(_) => 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_test_product_root(&self, root: rustls::pki_types::CertificateDer<'static>) {
        assert!(
            self.clients
                .set(product_clients(|| Ok(Trust::test(root.clone()))))
                .is_ok()
        );
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn set_product_fixtures(
        &self,
        fixtures: &[crate::testing::ProductFixture],
    ) -> Result<(), Box<Error>> {
        if fixtures.is_empty() {
            return Ok(());
        }
        let invalid = || Box::new(Error::new(ErrorClass::InvalidConfig));
        if fixtures.iter().any(|fixture| {
            !fixture
                .product
                .endpoint(self.endpoints())
                .is_some_and(|url| fixture.trust.allows(url))
        }) {
            return Err(invalid());
        }
        let clients = product_clients(|| {
            crate::wire::connection::FixtureTrust::trust(
                fixtures.iter().map(|fixture| &fixture.trust),
            )
        })
        .map_err(|_| invalid())?;
        self.clients.set(Ok(clients)).map_err(|_| invalid())?;
        self.product_fixtures
            .set(
                fixtures
                    .iter()
                    .map(|fixture| (fixture.product, fixture.write))
                    .collect(),
            )
            .map_err(|_| invalid())?;
        Ok(())
    }

    #[expect(
        clippy::result_large_err,
        reason = "The public API returns Error by value"
    )]
    pub(super) fn new(
        selected: credentials::Selected,
        region: &str,
        profile: String,
        project_id: Option<String>,
        directory: Option<PathBuf>,
        home: Option<PathBuf>,
        auth_options: (Option<IamUserAuth>, EndpointOverrides),
    ) -> Result<Self, Error> {
        let (iam_details, overrides) = auth_options;
        let invalid = || Error::new(ErrorClass::InvalidConfig);
        let details = if selected.source == credentials::Source::Options
            && matches!(&selected.kind, credentials::Kind::IamUser(_))
        {
            iam_details
        } else {
            None
        };
        let endpoints = endpoint::defaults(region, overrides.into()).map_err(|_| invalid())?;
        let default_endpoints =
            endpoint::defaults(region, Default::default()).map_err(|_| invalid())?;
        let mut endpoint_overrides_used = endpoints.overrides_used;
        let mut default_auth_endpoints = false;
        let directory = if matches!(
            &selected.kind,
            credentials::Kind::ServiceAccount(_) | credentials::Kind::IamUser(_)
        ) {
            directory
                .map(|path| crate::cache::checked_directory(home.as_deref(), &path).map_err(|e| *e))
                .transpose()?
        } else {
            None
        };
        if matches!(&selected.kind, credentials::Kind::ServiceAccount(_))
            && let Some(path) = &directory
        {
            crate::cache::checked_directory(home.as_deref(), &path.join("service-account-v1"))
                .map_err(|e| *e)?;
        }
        let clock: Arc<dyn crate::Clock> = Arc::new(crate::SystemClock);
        let timeout = Duration::from_secs(120);
        let mut configured_credentials = SentCredentials::default();
        if let Some(key) = &selected.cdn_key {
            configured_credentials.record(key);
        }
        match &selected.kind {
            credentials::Kind::StaticToken(token) => configured_credentials.record(token),
            credentials::Kind::ServiceAccount(account) => {
                configured_credentials.record(&account.client_secret)
            }
            credentials::Kind::IamUser(user) => {
                configured_credentials.record(&user.password);
                if let Some(seed) = &user.totp_secret {
                    configured_credentials.record(seed);
                    configured_credentials.record(
                        &crate::wire::credential::iam_user::totp::normalized_seed(seed),
                    );
                }
            }
            credentials::Kind::Provider(_) => {}
        }
        let auth = match selected.kind {
            credentials::Kind::Provider(provider) => Auth::Provider(provider),
            credentials::Kind::StaticToken(token) => Auth::Static(token),
            credentials::Kind::ServiceAccount(value) => Auth::ServiceAccount(Box::new(
                crate::session::service_account::ServiceAccount::new(
                    value.client_id.expose_secret().to_owned(),
                    value.client_secret,
                    profile.clone(),
                    directory,
                    home,
                    Arc::clone(&clock),
                    timeout,
                ),
            )),
            credentials::Kind::IamUser(value) => {
                default_auth_endpoints = true;
                let mut url = |base: &Option<Base>,
                               default: &Option<Base>,
                               supplied: Option<&str>|
                 -> Result<url::Url, Error> {
                    let supplied = supplied.filter(|value| !value.is_empty());
                    let base = base.as_ref().map(Base::configured_url);
                    let default = default.as_ref().map(Base::configured_url);
                    let value = supplied.or(base).ok_or_else(invalid)?;
                    endpoint_overrides_used |= supplied.is_some();
                    default_auth_endpoints &= base == default && Some(value) == default;
                    Base::parse(value).map_err(|_| invalid())?;
                    url::Url::parse(value).map_err(|_| invalid())
                };
                let signin = url(
                    &endpoints.signin,
                    &default_endpoints.signin,
                    details.as_ref().and_then(|v| v.signin_base_url.as_deref()),
                )?;
                let dashboard = url(
                    &endpoints.dashboard,
                    &default_endpoints.dashboard,
                    details.as_ref().and_then(|v| v.dashboard_uri.as_deref()),
                )?;
                let token_url = url(
                    &endpoints.token,
                    &default_endpoints.token,
                    details.as_ref().and_then(|v| v.token_url.as_deref()),
                )?;
                let provider = details.and_then(|v| v.provider);
                Auth::IamUser(Box::new(crate::session::iam_user::IamUser::new(
                    crate::session::iam_user::Options {
                        root_email: value.root_email.expose_secret().to_owned(),
                        username: value.username.expose_secret().to_owned(),
                        password: value.password,
                        seed: value.totp_secret,
                        provider,
                        profile: profile.clone(),
                        directory,
                        home,
                        signin,
                        dashboard,
                        token_url,
                        clock: Arc::clone(&clock),
                        timeout,
                    },
                )))
            }
        };
        Ok(Self {
            auth,
            configured_credentials,
            endpoints,
            default_endpoints,
            endpoint_overrides_used,
            default_auth_endpoints,
            #[cfg(any(test, feature = "test-support"))]
            product_fixtures: OnceLock::new(),
            clients: OnceLock::new(),
            clock,
            timeout,
            _profile: profile,
            region: region.to_owned(),
            configured_project_id: project_id.clone().filter(|id| !id.is_empty()),
            discovery: Discovery::new(project_id, region),
            cdn_key: selected.cdn_key,
        })
    }

    #[expect(
        clippy::result_large_err,
        reason = "The public API returns Error by value"
    )]
    pub(super) async fn authenticate(&self, ctx: &RequestContext) -> Result<(), Error> {
        if let Some(end) = ctx.check(&crate::SystemClock) {
            return Err(end.into());
        }
        match &self.auth {
            Auth::Static(token) => {
                let _ = token;
                Ok(())
            }
            Auth::ServiceAccount(source) => source.token(ctx).await.map(|_| ()),
            Auth::IamUser(source) => source.token(ctx).await.map(|_| ()),
            Auth::Provider(provider) => {
                use std::future::Future;
                use std::task::Poll;
                let mut ended = std::pin::pin!(ctx.wait(&crate::SystemClock));
                let mut work = provider.token(ctx);
                std::future::poll_fn(|cx| {
                    if let Poll::Ready(end) = ended.as_mut().poll(cx) {
                        return Poll::Ready(Err(end.into()));
                    }
                    match work.as_mut().poll(cx) {
                        Poll::Ready(result) => {
                            if let Some(end) = ctx.check(&crate::SystemClock) {
                                return Poll::Ready(Err(end.into()));
                            }
                            Poll::Ready(result.map(|_| ()).map_err(|_| Error::provider_failed()))
                        }
                        Poll::Pending => Poll::Pending,
                    }
                })
                .await
            }
        }
    }
}

fn product_clients(
    trust: impl Fn() -> Result<Trust, rustls::Error>,
) -> Result<Clients, rustls::Error> {
    Ok(Clients {
        pooled: PooledClient::new(HeadLimit::Api, trust()?)?,
        one_send: OneSendClient::new(HeadLimit::Api, trust()?)?,
    })
}

impl Sleeper for Session {
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.clock
            .sleep_until(self.clock.monotonic_now() + duration)
    }
}
