use std::any::Any;
use std::fmt;

#[path = "error/detail.rs"]
mod detail;
use detail::Detail;
pub(crate) use detail::debug_record;
pub(crate) use detail::{debug_reflects, write_debug};
#[path = "error/parts.rs"]
mod parts;
pub(crate) use parts::LoginFailureKind;
pub use parts::{ApiError, LoginError, ServiceAccountLoginError};

/// Stable categories for operation failures.
#[derive(Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorClass {
    /// A write outcome could not be confirmed.
    NotSettled,
    /// A write failed.
    Failed,
    /// Authentication failed.
    Auth,
    /// Token cache or TOTP marker is unavailable.
    TokenCache,
    /// A service request failed.
    Api,
    /// A resource was not found.
    NotFound,
    /// Access was denied.
    PermissionDenied,
    /// The service rate-limited the request.
    RateLimited,
    /// Project discovery found no project in the region.
    ProjectNotFound,
    /// Project discovery found multiple matching projects.
    ProjectAmbiguous,
    /// A required project identifier is missing.
    MissingProjectId,
    /// Configuration is invalid.
    InvalidConfig,
    /// Operation input is invalid.
    InvalidInput,
    /// No credentials were found; also classified as invalid configuration.
    NoCredentials,
    /// The credentials file could not be used; also classified as invalid configuration.
    CredentialsFile,
    /// History completeness could not be confirmed.
    HistoryIncomplete,
    /// Pricing discovery returned partial results.
    DiscoveryIncomplete,
    /// The estimate exhausted a request, row, or response limit.
    EstimateLimit,
    /// Local estimate arithmetic produced a non-finite amount.
    EstimateOverflow,
    /// The quote exceeded the allowed price; nothing was ordered.
    PriceAboveMax,
    /// The paid resource quote was zero; nothing was ordered.
    Unpriced,
    /// The operation was cancelled.
    Cancelled,
    /// The operation deadline was reached.
    DeadlineExceeded,
}

crate::error::debug_record!(ErrorClass;
    NotSettled,
    Failed,
    Auth,
    TokenCache,
    Api,
    NotFound,
    PermissionDenied,
    RateLimited,
    ProjectNotFound,
    ProjectAmbiguous,
    MissingProjectId,
    InvalidConfig,
    InvalidInput,
    NoCredentials,
    CredentialsFile,
    HistoryIncomplete,
    DiscoveryIncomplete,
    EstimateLimit,
    EstimateOverflow,
    PriceAboveMax,
    Unpriced,
    Cancelled,
    DeadlineExceeded,
);

#[derive(Debug)]
pub(crate) struct SafeText(String);

impl SafeText {
    pub(crate) fn new(text: &str, credentials: &[&crate::Secret]) -> Self {
        let cleaned: String = text.chars().filter(|c| {
            !c.is_control() && !matches!(*c, '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{206f}')
        }).collect();
        if credentials.iter().any(|credential| {
            text.contains(credential.expose_secret())
                || cleaned.contains(credential.expose_secret())
        }) {
            return Self(String::new());
        }
        let end = cleaned
            .char_indices()
            .map(|(i, c)| i + c.len_utf8())
            .take_while(|end| *end <= 512)
            .last()
            .unwrap_or(0);
        Self(cleaned[..end].to_owned())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Fixed diagnostic for a failure without a service response.
#[derive()]
pub enum Diagnostic {
    /// No project quota matched the requested name.
    QuotaNotFound,
}

crate::error::debug_record!(Diagnostic; QuotaNotFound);

impl Diagnostic {
    /// Returns the fixed diagnostic.
    pub fn message(&self) -> &'static str {
        match self {
            Self::QuotaNotFound => "greennode: not found: quota",
        }
    }
}

/// Safe recovery guidance for a write failure.
#[derive()]
pub struct RecoveryAdvice(SafeText);

crate::error::debug_record!(RecoveryAdvice(0));

impl RecoveryAdvice {
    pub(crate) fn new(message: &str) -> Self {
        Self(SafeText::new(message, &[]))
    }

    /// Returns the recovery guidance, which can include a sanitized service reason.
    pub fn message(&self) -> &str {
        self.0.as_str()
    }
}

/// Full refusal text for a system tag key, without the generic length cap.
#[derive()]
pub struct SystemTagText(pub(crate) String);

crate::error::debug_record!(SystemTagText(0));

impl SystemTagText {
    /// Returns the refusal text.
    pub fn message(&self) -> &str {
        &self.0
    }
}

/// HTTP response status retained in a screened SDK error.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct HttpStatus(http::StatusCode);

impl HttpStatus {
    /// Returns the numeric HTTP response status.
    pub fn as_u16(self) -> u16 {
        self.0.as_u16()
    }
}

crate::error::debug_record!(HttpStatus => |self, formatter| {
    formatter.write_str(self.0.as_str())
});

const DIAGNOSTIC_FALLBACK: &str = "request failed";

/// A categorized failure with optional typed details and a cause chain.
pub struct Error {
    class: ErrorClass,
    identity: bool,
    fallback_message_reflected: bool,
    display_reflected: bool,
    debug_reflected: bool,
    api: Option<ApiError>,
    login: Option<LoginError>,
    service_login: Option<ServiceAccountLoginError>,
    detail: Option<Detail>,
    cause: Option<Box<Error>>,
}

#[derive(Debug)]
struct BodyTooLarge;

impl Error {
    /// Creates an authentication failure for a custom credential provider.
    pub fn provider_failed() -> Self {
        Self::new(ErrorClass::Auth)
    }

    pub(crate) fn new(class: ErrorClass) -> Self {
        Self {
            class,
            identity: true,
            fallback_message_reflected: false,
            display_reflected: false,
            debug_reflected: false,
            api: None,
            login: None,
            service_login: None,
            detail: None,
            cause: if matches!(
                class,
                ErrorClass::NoCredentials | ErrorClass::CredentialsFile
            ) {
                Some(Box::new(Self::new(ErrorClass::InvalidConfig)))
            } else {
                None
            },
        }
    }
    pub(crate) fn from_public_api(mut api: ApiError) -> Self {
        api.public_route = true;
        Self::from_api(api)
    }
    pub(crate) fn from_api(api: ApiError) -> Self {
        let class = match api.status {
            Some(404) => ErrorClass::NotFound,
            Some(403) => ErrorClass::PermissionDenied,
            Some(429) => ErrorClass::RateLimited,
            Some(401) if !api.public_route => ErrorClass::Auth,
            _ => ErrorClass::Api,
        };
        Self {
            identity: false,
            api: Some(api),
            ..Self::new(class)
        }
    }
    pub(crate) fn screen_api(mut self, reflects: &impl Fn(&str) -> bool) -> Self {
        self = self.screen_diagnostic_fallback(reflects);
        if let Some(api) = self.api.as_mut() {
            if api
                .status
                .is_some_and(|status| reflects(&status.to_string()))
            {
                api.status = None;
                self.identity = true;
            }
            if reflects(api.operation) {
                api.operation = "";
            }
            if reflects(&api.code) {
                api.code.clear();
            }
            if reflects(&api.message) {
                api.message.clear();
            }
        }
        if let Some(login) = &mut self.login {
            login.screen(reflects);
        }
        if let Some(login) = &mut self.service_login {
            if reflects(login.reason) {
                login.reason = "";
            }
            if login
                .status
                .is_some_and(|status| reflects(&status.to_string()))
            {
                login.status = None;
            }
        }
        if let Some(detail) = &mut self.detail {
            detail.screen_fields(reflects);
        }
        self.cause = self.cause.map(|cause| Box::new(cause.screen_api(reflects)));
        self.display_reflected =
            self.display_reflected || reflects(&zeroize::Zeroizing::new(self.to_string()));
        self
    }

    pub(crate) fn screen_public(self, reflects: &impl Fn(&str) -> bool) -> Self {
        let mut error = self.screen_api(reflects);
        let mut node = Some(&mut error);
        while let Some(current) = node {
            if let Some(detail) = &mut current.detail {
                detail.screen_public(reflects);
            }
            if let Some(api) = &mut current.api {
                api.screen_formatter(reflects);
            }
            if let Some(login) = &mut current.login {
                login.screen_formatter(reflects);
            }
            if let Some(login) = &mut current.service_login {
                login.screen_formatter(reflects);
            }
            current.debug_reflected |= debug_reflects(current, reflects);
            node = current.cause.as_deref_mut();
        }
        error
    }

    pub(crate) fn screen_diagnostic_fallback(mut self, reflects: impl Fn(&str) -> bool) -> Self {
        self.fallback_message_reflected |= reflects(DIAGNOSTIC_FALLBACK);
        self
    }

    /// Returns the diagnostic fallback after credential screening.
    pub fn diagnostic_fallback(&self) -> &'static str {
        let mut error = Some(self);
        while let Some(node) = error {
            if node.fallback_message_reflected {
                return "";
            }
            error = node.cause.as_deref();
        }
        DIAGNOSTIC_FALLBACK
    }

    pub(crate) fn with_cause(mut self, cause: Error) -> Self {
        match self.cause.as_mut() {
            Some(existing) => {
                let mut node = existing.as_mut();
                while let Some(ref mut next) = node.cause {
                    node = next;
                }
                node.cause = Some(Box::new(cause));
            }
            None => self.cause = Some(Box::new(cause)),
        }
        self
    }
    pub(crate) fn with_login(mut self, detail: LoginError) -> Self {
        self.login = Some(detail);
        self
    }
    pub(crate) fn with_service_login(mut self, detail: ServiceAccountLoginError) -> Self {
        self.service_login = Some(detail);
        self
    }
    pub(crate) fn with_http_status(self, status: http::StatusCode) -> Self {
        self.with_outer_detail(HttpStatus(status))
    }

    pub(crate) fn http_status(&self) -> Option<http::StatusCode> {
        self.detail::<HttpStatus>().map(|status| status.0)
    }

    pub(crate) fn with_outer_detail<T: Any + Send + Sync + fmt::Debug>(self, detail: T) -> Self {
        Self {
            class: self.class,
            identity: false,
            fallback_message_reflected: false,
            display_reflected: false,
            debug_reflected: false,
            api: None,
            login: None,
            service_login: None,
            detail: Some(Detail::new(detail)),
            cause: Some(Box::new(self)),
        }
    }
    pub(crate) fn is_body_too_large(&self) -> bool {
        self.detail::<BodyTooLarge>().is_some()
    }
    pub(crate) fn with_body_too_large(self) -> Self {
        self.with_outer_detail(BodyTooLarge)
    }
    pub(crate) fn with_detail<T: Any + Send + Sync + fmt::Debug>(mut self, detail: T) -> Self {
        self.detail = Some(Detail::new(detail));
        self
    }
    pub(crate) fn remove_details<T: Any + Send + Sync>(&mut self, matches: impl Fn(&T) -> bool) {
        let mut error = Some(self);
        while let Some(node) = error {
            if node
                .detail
                .as_ref()
                .and_then(|detail| detail.value().downcast_ref())
                .is_some_and(&matches)
            {
                node.detail = None;
            }
            error = node.cause.as_deref_mut();
        }
    }
    /// Returns the outermost category.
    pub fn class(&self) -> ErrorClass {
        self.class
    }
    /// Checks the cause chain and maps HTTP 401, 403, 404 and 429 to their categories.
    /// Public routes carry no credential for a 401 to reject. Any API error also matches `Api`.
    pub fn has_class(&self, class: ErrorClass) -> bool {
        self.has_identity(class)
            || self.api_error().is_some_and(|api| match class {
                ErrorClass::NotFound => api.status == Some(404),
                ErrorClass::PermissionDenied => api.status == Some(403),
                ErrorClass::RateLimited => api.status == Some(429),
                ErrorClass::Auth => api.status == Some(401) && !api.public_route,
                ErrorClass::Api => true,
                _ => false,
            })
    }
    /// Checks explicit category identities in the cause chain without HTTP status mapping.
    pub fn has_identity(&self, class: ErrorClass) -> bool {
        (self.identity && self.class == class)
            || self
                .cause
                .as_ref()
                .is_some_and(|cause| cause.has_identity(class))
    }
    /// Returns the first service error in the cause chain, if present.
    pub fn api_error(&self) -> Option<&ApiError> {
        self.api
            .as_ref()
            .or_else(|| self.cause.as_ref().and_then(|cause| cause.api_error()))
    }
    /// Returns the first IAM login detail in the cause chain, if present.
    pub fn login_error(&self) -> Option<&LoginError> {
        self.login
            .as_ref()
            .or_else(|| self.cause.as_ref().and_then(|cause| cause.login_error()))
    }
    /// Returns the first service-account login detail in the cause chain, if present.
    pub fn service_account_login_error(&self) -> Option<&ServiceAccountLoginError> {
        self.service_login.as_ref().or_else(|| {
            self.cause
                .as_ref()
                .and_then(|cause| cause.service_account_login_error())
        })
    }
    /// Returns the first matching typed detail in the cause chain, if present.
    pub fn detail<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.detail
            .as_ref()
            .and_then(Detail::public)
            .or_else(|| self.cause.as_ref().and_then(|cause| cause.detail::<T>()))
    }
}
impl From<crate::ContextEnd> for Error {
    fn from(end: crate::ContextEnd) -> Self {
        Self::new(match end {
            crate::ContextEnd::Cancelled => ErrorClass::Cancelled,
            crate::ContextEnd::DeadlineExceeded => ErrorClass::DeadlineExceeded,
        })
    }
}

crate::error::debug_record!(Error => |self, f| {
    if self.debug_reflected {
        return f.write_str("");
    }
    f.debug_struct("Error")
        .field("class", &self.class)
        .finish_non_exhaustive()
});
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.display_reflected {
            return f.write_str(self.diagnostic_fallback());
        }
        write!(f, "greennode: {:?}", self.class)
    }
}
impl std::error::Error for Error {}

/// Whether a failed operation may have dispatched a write.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum WriteDispatch {
    /// No write was attempted.
    NotAWrite,
    /// The write was not sent.
    NotSent,
    /// The write may have reached the service; its outcome is uncertain.
    MayHaveBeenSent,
}

crate::error::debug_record!(WriteDispatch; NotAWrite, NotSent, MayHaveBeenSent);

/// A failure that may retain partial output and write-dispatch state.
pub struct Failure<T> {
    error: Error,
    partial: Option<T>,
    dispatch: WriteDispatch,
    private_delivery: Option<Box<crate::Secret>>,
    debug_reflected: bool,
}
impl<T> Failure<T> {
    pub(crate) fn new(error: Error, partial: Option<T>, dispatch: WriteDispatch) -> Self {
        Self {
            error,
            partial,
            dispatch,
            private_delivery: None,
            debug_reflected: false,
        }
    }
    pub(crate) fn with_private_delivery(mut self, secret: crate::Secret) -> Self {
        self.private_delivery = Some(Box::new(secret));
        self
    }
    pub(crate) fn private_delivery(&self) -> Option<&crate::Secret> {
        self.private_delivery.as_deref()
    }
    pub(crate) fn wrap<U>(
        error: Error,
        partial: Option<T>,
        dispatch: WriteDispatch,
        inner: Failure<U>,
    ) -> Self {
        let dispatch = if inner.dispatch == WriteDispatch::MayHaveBeenSent {
            WriteDispatch::MayHaveBeenSent
        } else {
            dispatch
        };
        Self::new(error.with_cause(inner.error), partial, dispatch)
    }
    pub(crate) fn screen_exit(
        mut self,
        screen_error: impl FnOnce(Error) -> Error,
        reflects: impl FnOnce(&T) -> bool,
        private_reflects: impl FnOnce(&crate::Secret) -> bool,
        reflects_debug: impl Fn(&str) -> bool,
    ) -> Self {
        self.error = screen_error(self.error);
        if self.partial.as_ref().is_some_and(reflects) {
            self.partial = None;
        }
        if self
            .private_delivery
            .as_deref()
            .is_some_and(private_reflects)
        {
            self.private_delivery = None;
        }
        self.debug_reflected |= debug_reflects(&self, &reflects_debug);
        self
    }
    /// Returns the categorized error.
    pub fn error(&self) -> &Error {
        &self.error
    }
    /// Returns partial output when the operation produced it before failing.
    pub fn partial(&self) -> Option<&T> {
        self.partial.as_ref()
    }
    /// Reports whether a write may have been sent.
    pub fn write_dispatch(&self) -> WriteDispatch {
        self.dispatch
    }
    /// Consumes the failure and returns its error, partial output, and dispatch state.
    /// Any separate private delivery is dropped.
    pub fn into_parts(self) -> (Error, Option<T>, WriteDispatch) {
        (self.error, self.partial, self.dispatch)
    }
}
crate::error::debug_record!(Failure<T> => |self, f| {
    if self.debug_reflected {
        return f.write_str("");
    }
    f.debug_struct("Failure")
        .field("error", &self.error)
        .field("has_partial", &self.partial.is_some())
        .field("write_dispatch", &self.dispatch)
        .finish()
});
