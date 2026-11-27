use super::SafeText;
use std::fmt;

/// Sanitized details of a failed service request.
pub struct ApiError {
    debug_reflected: bool,
    pub(super) operation: &'static str,
    pub(super) status: Option<u16>,
    pub(super) code: String,
    pub(super) message: String,
    pub(super) retryable: bool,
    pub(super) public_route: bool,
}

impl ApiError {
    pub(super) fn screen_formatter(&mut self, reflects: &impl Fn(&str) -> bool) {
        self.debug_reflected |= super::detail::debug_reflects(self, reflects);
    }

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
            debug_reflected: false,
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
            debug_reflected: false,
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
        super::detail::write_debug(f, |f| {
            if self.debug_reflected {
                return f.write_str("");
            }
            f.debug_struct("ApiError")
                .field("operation", &self.operation)
                .field("status", &self.status)
                .field("retryable", &self.retryable)
                .finish_non_exhaustive()
        })
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
    debug_reflected: bool,
    kind: LoginFailureKind,
    pub(super) reason: &'static str,
    pub(super) status: Option<u16>,
    pub(super) captcha_suspected: bool,
}
impl LoginError {
    pub(super) fn screen_formatter(&mut self, reflects: &impl Fn(&str) -> bool) {
        self.debug_reflected |= super::detail::debug_reflects(self, reflects);
    }

    pub(crate) fn new(reason: &'static str, status: Option<u16>, captcha_suspected: bool) -> Self {
        Self {
            debug_reflected: false,
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
        super::detail::write_debug(f, |f| {
            if self.debug_reflected {
                return f.write_str("");
            }
            f.debug_struct("LoginError")
                .field("status", &self.status)
                .field("captcha_suspected", &self.captcha_suspected)
                .finish_non_exhaustive()
        })
    }
}

/// Safe details of a service-account token exchange failure.
pub struct ServiceAccountLoginError {
    debug_reflected: bool,
    pub(super) reason: &'static str,
    pub(super) status: Option<u16>,
}
impl ServiceAccountLoginError {
    pub(super) fn screen_formatter(&mut self, reflects: &impl Fn(&str) -> bool) {
        self.debug_reflected |= super::detail::debug_reflects(self, reflects);
    }

    pub(crate) fn new(reason: &'static str, status: Option<u16>) -> Self {
        Self {
            reason,
            status,
            debug_reflected: false,
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
}
impl fmt::Debug for ServiceAccountLoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        super::detail::write_debug(f, |f| {
            if self.debug_reflected {
                return f.write_str("");
            }
            f.debug_struct("ServiceAccountLoginError")
                .field("status", &self.status)
                .finish_non_exhaustive()
        })
    }
}
