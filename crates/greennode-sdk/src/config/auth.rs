use crate::{Clock, Error, RequestContext, Secret};
use std::{future::Future, pin::Pin, sync::Arc};

/// Credentials for a service-account login.
#[derive(Clone)]
pub struct ServiceAccountAuth {
    pub(super) client_id: Secret,
    pub(super) client_secret: Secret,
}

crate::error::debug_record!(ServiceAccountAuth => |self, f| {
    f.write_str("ServiceAccountAuth([REDACTED])")
});

impl ServiceAccountAuth {
    /// Creates a service-account credential snapshot.
    pub fn new(client_id: String, client_secret: Secret) -> Self {
        Self {
            client_id: Secret::new(client_id),
            client_secret,
        }
    }
}

/// Supplies a one-time code for an IAM user login.
pub trait TotpProvider: Send + Sync {
    /// Returns a code while respecting the request context.
    fn code<'a>(
        &'a self,
        ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Secret, Error>> + Send + 'a>>;
}

/// Adapts an asynchronous function to a TOTP provider.
pub struct TotpFn<F>(F);

impl<F> TotpFn<F> {
    /// Wraps an asynchronous code function.
    pub fn new(function: F) -> Self {
        Self(function)
    }
}

impl<F> TotpProvider for TotpFn<F>
where
    F: for<'a> Fn(
            &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Secret, Error>> + Send + 'a>>
        + Send
        + Sync,
{
    fn code<'a>(
        &'a self,
        ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Secret, Error>> + Send + 'a>> {
        (self.0)(ctx)
    }
}

/// Generates time-based one-time codes from a secret seed.
#[derive(Clone)]
pub struct SecretTotp(Secret);

crate::error::debug_record!(SecretTotp(0));

impl SecretTotp {
    /// Creates a provider from a base32-encoded seed.
    pub fn new(seed: Secret) -> Self {
        Self(seed)
    }
}

impl TotpProvider for SecretTotp {
    fn code<'a>(
        &'a self,
        ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Secret, Error>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(end) = ctx.check(&crate::SystemClock) {
                return Err(end.into());
            }
            crate::wire::credential::iam_user::totp::code(
                &self.0,
                crate::SystemClock.utc_now().unix_timestamp(),
            )
            .map_err(|_| Error::provider_failed())
        })
    }
}

/// Credentials and optional destinations for an IAM-user login.
#[derive(Clone)]
pub struct IamUserAuth {
    pub(super) root_email: String,
    pub(super) username: String,
    pub(super) password: Secret,
    pub(super) provider: Option<Arc<dyn TotpProvider>>,
    pub(super) signin_base_url: Option<String>,
    pub(super) token_url: Option<String>,
    pub(super) dashboard_uri: Option<String>,
}

crate::error::debug_record!(IamUserAuth => |self, f| {
    f.write_str("IamUserAuth([REDACTED])")
});

impl IamUserAuth {
    /// Creates an IAM-user credential snapshot.
    pub fn new(root_email: String, username: String, password: Secret) -> Self {
        Self {
            root_email,
            username,
            password,
            provider: None,
            signin_base_url: None,
            token_url: None,
            dashboard_uri: None,
        }
    }
    /// Supplies an asynchronous one-time-code provider.
    pub fn with_totp_provider(mut self, provider: Arc<dyn TotpProvider>) -> Self {
        self.provider = Some(provider);
        self
    }
    /// Overrides the IAM sign-in base URL.
    pub fn with_signin_base_url(mut self, url: String) -> Self {
        self.signin_base_url = Some(url);
        self
    }
    /// Overrides the IAM token URL.
    pub fn with_token_url(mut self, url: String) -> Self {
        self.token_url = Some(url);
        self
    }
    /// Overrides the IAM dashboard URL.
    pub fn with_dashboard_uri(mut self, url: String) -> Self {
        self.dashboard_uri = Some(url);
        self
    }
}
