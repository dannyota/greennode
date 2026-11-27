use std::any::Any;
use std::fmt;

/// Stable categories for operation failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
pub enum Diagnostic {
    /// No project quota matched the requested name.
    QuotaNotFound,
}

impl Diagnostic {
    /// Returns the fixed diagnostic.
    pub fn message(&self) -> &'static str {
        match self {
            Self::QuotaNotFound => "greennode: not found: quota",
        }
    }
}

/// Safe recovery guidance for a write failure.
pub struct RecoveryAdvice(SafeText);

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
pub struct SystemTagText(pub(crate) String);

impl SystemTagText {
    /// Returns the refusal text.
    pub fn message(&self) -> &str {
        &self.0
    }
}

/// Sanitized details of a failed service request.
pub struct ApiError {
    operation: &'static str,
    status: Option<u16>,
    code: String,
    message: String,
    retryable: bool,
    public_route: bool,
}

impl ApiError {
    pub(crate) fn new(
        operation: &'static str,
        status: Option<u16>,
        code: SafeText,
        message: SafeText,
        retryable: bool,
    ) -> Self {
        let value = code.as_str();
        let code =
            if value.is_empty() || value == "null" || value == status.unwrap_or(0).to_string() {
                match status {
                    Some(400) => "BadRequest",
                    Some(401) => "Unauthorized",
                    Some(403) => "Forbidden",
                    Some(404) => "NotFound",
                    Some(409) => "Conflict",
                    Some(429) => "Throttled",
                    Some(500..) => "ServerError",
                    Some(400..) => "ClientError",
                    _ => "",
                }
            } else {
                value
            };
        Self {
            operation,
            status,
            code: code.to_owned(),
            message: message.0,
            retryable,
            public_route: false,
        }
    }
    pub(crate) fn without_fallback(
        operation: &'static str,
        status: Option<u16>,
        code: SafeText,
        message: SafeText,
        retryable: bool,
    ) -> Self {
        Self {
            operation,
            status,
            code: code.0,
            message: message.0,
            retryable,
            public_route: false,
        }
    }
    /// Returns the operation identifier.
    pub fn operation(&self) -> &'static str {
        self.operation
    }
    /// Returns the HTTP status when a response was received.
    pub fn status(&self) -> Option<u16> {
        self.status
    }
    /// Returns the sanitized service code or a status-derived fallback, possibly empty.
    pub fn code(&self) -> &str {
        &self.code
    }
    /// Returns the sanitized service message, possibly empty.
    pub fn message(&self) -> &str {
        &self.message
    }
    /// Reports whether the service failure is marked retryable.
    pub fn retryable(&self) -> bool {
        self.retryable
    }
}

impl fmt::Debug for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiError")
            .field("operation", &self.operation)
            .field("status", &self.status)
            .field("retryable", &self.retryable)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum LoginFailureKind {
    Other,
    SigninRejected,
    TotpRejected,
    TokenCache,
}

/// Safe details of an IAM user login failure.
#[derive(Clone)]
pub struct LoginError {
    kind: LoginFailureKind,
    reason: &'static str,
    status: Option<u16>,
    captcha_suspected: bool,
}
impl LoginError {
    pub(crate) fn new(reason: &'static str, status: Option<u16>, captcha_suspected: bool) -> Self {
        Self {
            kind: LoginFailureKind::Other,
            reason,
            status,
            captcha_suspected,
        }
    }

    pub(crate) fn with_kind(mut self, kind: LoginFailureKind) -> Self {
        self.kind = kind;
        self
    }

    pub(crate) fn kind(&self) -> LoginFailureKind {
        self.kind
    }

    pub(crate) fn screen(&mut self, reflects: impl Fn(&str) -> bool) {
        if reflects(self.reason) {
            self.reason = "";
        }
        if self
            .status
            .is_some_and(|status| reflects(&status.to_string()))
        {
            self.status = None;
        }
    }

    /// Returns fixed text identifying the failed step.
    pub fn reason(&self) -> &'static str {
        self.reason
    }
    /// Returns the HTTP status when a response was received.
    pub fn status(&self) -> Option<u16> {
        self.status
    }
    /// Reports a redisplayed sign-in form, which may indicate a CAPTCHA or wrong credentials.
    pub fn captcha_suspected(&self) -> bool {
        self.captcha_suspected
    }
}
impl fmt::Debug for LoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginError")
            .field("status", &self.status)
            .field("captcha_suspected", &self.captcha_suspected)
            .finish_non_exhaustive()
    }
}

/// Safe details of a service-account token exchange failure.
pub struct ServiceAccountLoginError {
    reason: &'static str,
    status: Option<u16>,
}
impl ServiceAccountLoginError {
    pub(crate) fn new(reason: &'static str, status: Option<u16>) -> Self {
        Self { reason, status }
    }
    /// Returns fixed text identifying the failed step.
    pub fn reason(&self) -> &'static str {
        self.reason
    }
    /// Returns the HTTP status when a response was received.
    pub fn status(&self) -> Option<u16> {
        self.status
    }
}
impl fmt::Debug for ServiceAccountLoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceAccountLoginError")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

const DIAGNOSTIC_FALLBACK: &str = "request failed";

/// A categorized failure with optional typed details and a cause chain.
pub struct Error {
    class: ErrorClass,
    identity: bool,
    fallback_message_reflected: bool,
    api: Option<ApiError>,
    login: Option<LoginError>,
    service_login: Option<ServiceAccountLoginError>,
    detail: Option<Box<dyn Any + Send + Sync>>,
    cause: Option<Box<Error>>,
}

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
            class,
            identity: false,
            fallback_message_reflected: false,
            api: Some(api),
            login: None,
            service_login: None,
            detail: None,
            cause: None,
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
        if let Some(detail) = self.detail.as_mut() {
            if let Some(text) = detail.downcast_mut::<&'static str>()
                && reflects(text)
            {
                *text = "";
            }
            if let Some(text) = detail.downcast_mut::<String>()
                && reflects(text)
            {
                text.clear();
            }
            if let Some(advice) = detail.downcast_mut::<RecoveryAdvice>()
                && reflects(advice.message())
            {
                advice.0.0.clear();
            }
            if let Some(text) = detail.downcast_mut::<SystemTagText>()
                && reflects(text.message())
            {
                text.0.clear();
            }
            if let Some(diagnostic) = detail.downcast_ref::<Diagnostic>()
                && reflects(diagnostic.message())
            {
                self.detail = None;
            }
        }
        if self
            .detail
            .as_ref()
            .and_then(|detail| detail.downcast_ref::<http::StatusCode>())
            .is_some_and(|status| reflects(status.as_str()))
        {
            self.detail = None;
        }
        self.cause = self.cause.map(|cause| Box::new(cause.screen_api(reflects)));
        self
    }

    pub(crate) fn screen_diagnostic_fallback(mut self, reflects: impl Fn(&str) -> bool) -> Self {
        self.fallback_message_reflected |= reflects(DIAGNOSTIC_FALLBACK);
        self
    }

    /// Returns the diagnostic fallback after credential screening.
    pub fn diagnostic_fallback(&self) -> &'static str {
        self.screened_diagnostic_text(DIAGNOSTIC_FALLBACK)
    }

    /// Withholds a fixed CLI diagnostic that matches evidence recorded for this failure.
    pub fn screened_diagnostic_text<'a>(&self, text: &'a str) -> &'a str {
        if text != DIAGNOSTIC_FALLBACK {
            return text;
        }
        let mut error = Some(self);
        while let Some(node) = error {
            if node.fallback_message_reflected {
                return "";
            }
            error = node.cause.as_deref();
        }
        text
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
    pub(crate) fn with_outer_detail<T: Any + Send + Sync>(self, detail: T) -> Self {
        Self {
            class: self.class,
            identity: false,
            fallback_message_reflected: false,
            api: None,
            login: None,
            service_login: None,
            detail: Some(Box::new(detail)),
            cause: Some(Box::new(self)),
        }
    }
    pub(crate) fn is_body_too_large(&self) -> bool {
        self.detail::<BodyTooLarge>().is_some()
    }
    pub(crate) fn with_body_too_large(self) -> Self {
        self.with_outer_detail(BodyTooLarge)
    }
    pub(crate) fn with_detail<T: Any + Send + Sync>(mut self, detail: T) -> Self {
        self.detail = Some(Box::new(detail));
        self
    }
    pub(crate) fn remove_details<T: Any + Send + Sync>(&mut self, matches: impl Fn(&T) -> bool) {
        let mut error = Some(self);
        while let Some(node) = error {
            if node
                .detail
                .as_ref()
                .and_then(|detail| detail.downcast_ref())
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
            .and_then(|detail| detail.downcast_ref())
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

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Error")
            .field("class", &self.class)
            .finish_non_exhaustive()
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "greennode: {:?}", self.class)
    }
}
impl std::error::Error for Error {}

/// Whether a failed operation may have dispatched a write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteDispatch {
    /// No write was attempted.
    NotAWrite,
    /// The write was not sent.
    NotSent,
    /// The write may have reached the service; its outcome is uncertain.
    MayHaveBeenSent,
}

/// A failure that may retain partial output and write-dispatch state.
pub struct Failure<T> {
    error: Error,
    partial: Option<T>,
    dispatch: WriteDispatch,
    private_delivery: Option<Box<crate::Secret>>,
}
impl<T> Failure<T> {
    pub(crate) fn new(error: Error, partial: Option<T>, dispatch: WriteDispatch) -> Self {
        Self {
            error,
            partial,
            dispatch,
            private_delivery: None,
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
impl<T> fmt::Debug for Failure<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Failure")
            .field("error", &self.error)
            .field("has_partial", &self.partial.is_some())
            .field("write_dispatch", &self.dispatch)
            .finish()
    }
}
