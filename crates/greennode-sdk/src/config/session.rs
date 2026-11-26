#[cfg(test)]
mod tests;

use super::{EndpointOverrides, IamUserAuth, credentials};
use crate::wire::connection::{HeadLimit, OneSendClient, PooledClient, Trust};
use crate::wire::product::sender::{BearerSource, Clients, Runtime, Sleeper};
use crate::{Clock, Error, ErrorClass, RequestContext, Secret};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::{path::PathBuf, time::Duration};

pub(crate) struct Session {
    auth: Auth,
    _endpoints: crate::endpoint::EndpointSet,
    clients: OnceLock<Result<Clients, rustls::Error>>,
    clock: Arc<dyn Clock>,
    timeout: Duration,
    _profile: String,
    _project_id: Option<String>,
    _cdn_key: Option<Secret>,
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

impl Session {
    pub(crate) fn endpoints(&self) -> &crate::endpoint::EndpointSet {
        &self._endpoints
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
            .get_or_init(|| product_clients(Trust::webpki))
            .as_ref()
            .map_err(|_| Box::new(Error::new(ErrorClass::InvalidConfig)))?;
        Ok(Runtime {
            client,
            ctx,
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
                .set(product_clients(|| Trust::test(root.clone())))
                .is_ok()
        );
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
        let endpoints =
            crate::endpoint::defaults(region, overrides.into()).map_err(|_| invalid())?;
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
                let url = |base: &Option<crate::endpoint::Base>,
                           override_value: Option<&str>|
                 -> Result<url::Url, Error> {
                    let value = override_value
                        .filter(|value| !value.is_empty())
                        .or(base.as_deref())
                        .ok_or_else(invalid)?;
                    crate::endpoint::Base::parse(value).map_err(|_| invalid())?;
                    url::Url::parse(value).map_err(|_| invalid())
                };
                let signin = url(
                    &endpoints.signin,
                    details.as_ref().and_then(|v| v.signin_base_url.as_deref()),
                )?;
                let dashboard = url(
                    &endpoints.dashboard,
                    details.as_ref().and_then(|v| v.dashboard_uri.as_deref()),
                )?;
                let token_url = url(
                    &endpoints.token,
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
            _endpoints: endpoints,
            clients: OnceLock::new(),
            clock,
            timeout,
            _profile: profile,
            _project_id: project_id,
            _cdn_key: selected.cdn_key,
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

fn product_clients(trust: impl Fn() -> Trust) -> Result<Clients, rustls::Error> {
    Ok(Clients {
        pooled: PooledClient::new(HeadLimit::Api, trust())?,
        one_send: OneSendClient::new(HeadLimit::Api, trust())?,
    })
}

impl Sleeper for Session {
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.clock
            .sleep_until(self.clock.monotonic_now() + duration)
    }
}
