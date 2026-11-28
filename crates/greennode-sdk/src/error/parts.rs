use super::{ApiProvenance, SafeText};

/// Sanitized details of a failed service request.
#[derive(Clone)]
pub struct ApiError {
    debug_reflected: bool,
    pub(super) operation: &'static str,
    pub(super) status: Option<u16>,
    pub(super) code: String,
    pub(super) invalid_response: bool,
    pub(super) message: String,
    pub(super) retryable: bool,
    pub(super) public_route: bool,
    pub(super) provenance: ApiProvenance,
    pub(super) request_cause: Option<String>,
    pub(super) message_is_formatted: bool,
    pub(super) status_withheld: bool,
    pub(super) operation_withheld: bool,
    pub(super) display: super::display::DisplayOverride,
}

impl ApiError {
    pub(super) fn screen_formatter(&mut self, reflects: &impl Fn(&str) -> bool) {
        self.debug_reflected |= super::detail::debug_reflects(self, reflects);
    }

    #[allow(dead_code)]
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
                Self::fallback_code(status)
            } else {
                value
            };
        Self {
            debug_reflected: false,
            operation,
            status,
            code: code.to_owned(),
            invalid_response: code == "InvalidResponse",
            message: message.0,
            retryable,
            public_route: false,
            provenance: ApiProvenance::from_fields(operation, status),
            request_cause: None,
            message_is_formatted: false,
            status_withheld: false,
            operation_withheld: false,
            display: Default::default(),
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
            invalid_response: code.as_str() == "InvalidResponse",
            code: code.0,
            message: message.0,
            retryable,
            public_route: false,
            provenance: ApiProvenance::from_fields(operation, status),
            request_cause: None,
            message_is_formatted: false,
            status_withheld: false,
            operation_withheld: false,
            display: Default::default(),
        }
    }
    pub(crate) fn with_formatted_message(mut self) -> Self {
        self.message_is_formatted = true;
        self
    }

    pub(crate) fn with_request_cause(mut self, cause: SafeText) -> Self {
        self.request_cause = Some(cause.0);
        if self.provenance != ApiProvenance::BareOverflow {
            self.provenance = ApiProvenance::Transport;
        }
        self
    }

    pub(crate) fn is_client_rejection(&self) -> bool {
        self.observed_response_status()
            .is_some_and(|status| (400..500).contains(&status))
    }

    pub(crate) fn with_public_status_withheld(mut self) -> Self {
        self.status_withheld = true;
        self
    }

    pub(crate) fn observed_status(&self) -> Option<u16> {
        self.status
    }

    pub(crate) fn is_for_operation(&self, operation: &str) -> bool {
        self.operation == operation
    }

    pub(crate) fn go_text(&self) -> String {
        self.to_string()
    }

    pub(crate) fn with_status_evidence(mut self) -> Self {
        self.provenance = ApiProvenance::BareOverflow;
        self
    }
    /// Returns the operation identifier.
    pub fn operation(&self) -> &'static str {
        if self.operation_withheld {
            ""
        } else {
            self.operation
        }
    }
    /// Returns the public HTTP status. `None` also covers a received status withheld
    /// by credential screening. A withheld status can still affect classification.
    pub fn status(&self) -> Option<u16> {
        self.status.filter(|_| !self.status_withheld)
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

crate::error::debug_record!(ApiError => |self, f| {
    super::detail::write_debug(f, |f| {
        if self.debug_reflected {
            return f.write_str("");
        }
        f.debug_struct("ApiError")
            .field("operation", &self.operation())
            .field("status", &self.status())
            .field("retryable", &self.retryable)
            .finish_non_exhaustive()
    })
});

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
crate::error::debug_record!(LoginError => |self, f| {
    super::detail::write_debug(f, |f| {
        if self.debug_reflected {
            return f.write_str("");
        }
        f.debug_struct("LoginError")
            .field("status", &self.status())
            .field("captcha_suspected", &self.captcha_suspected)
            .finish_non_exhaustive()
    })
});

/// Safe details of a service-account token exchange failure.
#[derive(Clone)]
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
crate::error::debug_record!(ServiceAccountLoginError => |self, f| {
    super::detail::write_debug(f, |f| {
        if self.debug_reflected {
            return f.write_str("");
        }
        f.debug_struct("ServiceAccountLoginError")
            .field("status", &self.status())
            .finish_non_exhaustive()
    })
});
