#[cfg(test)]
mod tests;

use super::{IamUserAuth, credentials};
use crate::wire::product::sender::BearerSource;
use crate::{Error, ErrorClass, RequestContext, Secret};
use std::{path::PathBuf, sync::Arc, time::Duration};

pub(super) struct Session {
    auth: Auth,
    _endpoints: crate::endpoint::EndpointSet,
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

impl Session {
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
        iam_details: Option<IamUserAuth>,
    ) -> Result<Self, Error> {
        let invalid = || Error::new(ErrorClass::InvalidConfig);
        let details = if selected.source == credentials::Source::Options
            && matches!(&selected.kind, credentials::Kind::IamUser(_))
        {
            iam_details
        } else {
            None
        };
        let endpoints =
            crate::endpoint::defaults(region, Default::default()).map_err(|_| invalid())?;
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
                    clock,
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
                        clock,
                        timeout,
                    },
                )))
            }
        };
        Ok(Self {
            auth,
            _endpoints: endpoints,
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
