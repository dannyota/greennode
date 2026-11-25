use std::any::Any;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorClass {
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
    PriceAboveMax,
    Unpriced,
    Cancelled,
    DeadlineExceeded,
}

pub struct ApiError {
    operation: &'static str,
    status: Option<u16>,
    code: String,
    message: String,
    retryable: bool,
}

impl ApiError {
    #[allow(dead_code)]
    pub(crate) fn new(
        operation: &'static str,
        status: Option<u16>,
        code: &str,
        message: &str,
        retryable: bool,
    ) -> Self {
        let code =
            if code.is_empty() || code == "null" || status.is_some_and(|s| code == s.to_string()) {
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
                code
            };
        Self {
            operation,
            status,
            code: code.to_owned(),
            message: message.to_owned(),
            retryable,
        }
    }
    pub fn operation(&self) -> &'static str {
        self.operation
    }
    pub fn status(&self) -> Option<u16> {
        self.status
    }
    pub fn code(&self) -> &str {
        &self.code
    }
    pub fn message(&self) -> &str {
        &self.message
    }
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

pub struct LoginError {
    reason: &'static str,
    status: Option<u16>,
    captcha_suspected: bool,
}
impl LoginError {
    #[allow(dead_code)]
    pub(crate) fn new(reason: &'static str, status: Option<u16>, captcha_suspected: bool) -> Self {
        Self {
            reason,
            status,
            captcha_suspected,
        }
    }
    pub fn reason(&self) -> &'static str {
        self.reason
    }
    pub fn status(&self) -> Option<u16> {
        self.status
    }
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

pub struct ServiceAccountLoginError {
    reason: &'static str,
    status: Option<u16>,
}
impl ServiceAccountLoginError {
    #[allow(dead_code)]
    pub(crate) fn new(reason: &'static str, status: Option<u16>) -> Self {
        Self { reason, status }
    }
    pub fn reason(&self) -> &'static str {
        self.reason
    }
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

pub struct Error {
    class: ErrorClass,
    identity: bool,
    api: Option<ApiError>,
    login: Option<LoginError>,
    service_login: Option<ServiceAccountLoginError>,
    detail: Option<Box<dyn Any + Send + Sync>>,
    cause: Option<Box<Error>>,
}

#[allow(dead_code)]
impl Error {
    pub(crate) fn new(class: ErrorClass) -> Self {
        Self {
            class,
            identity: true,
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
    pub(crate) fn from_api(api: ApiError) -> Self {
        let class = match api.status {
            Some(404) => ErrorClass::NotFound,
            Some(403) => ErrorClass::PermissionDenied,
            Some(429) => ErrorClass::RateLimited,
            Some(401) => ErrorClass::Auth,
            _ => ErrorClass::Api,
        };
        Self {
            class,
            identity: false,
            api: Some(api),
            login: None,
            service_login: None,
            detail: None,
            cause: None,
        }
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
    pub(crate) fn with_detail<T: Any + Send + Sync>(mut self, detail: T) -> Self {
        self.detail = Some(Box::new(detail));
        self
    }
    pub fn class(&self) -> ErrorClass {
        self.class
    }
    pub fn has_class(&self, class: ErrorClass) -> bool {
        self.has_identity(class)
            || self.api_error().is_some_and(|api| match class {
                ErrorClass::NotFound => api.status == Some(404),
                ErrorClass::PermissionDenied => api.status == Some(403),
                ErrorClass::RateLimited => api.status == Some(429),
                ErrorClass::Auth => api.status == Some(401),
                ErrorClass::Api => true,
                _ => false,
            })
    }
    pub fn has_identity(&self, class: ErrorClass) -> bool {
        (self.identity && self.class == class)
            || self
                .cause
                .as_ref()
                .is_some_and(|cause| cause.has_identity(class))
    }
    pub fn api_error(&self) -> Option<&ApiError> {
        self.api
            .as_ref()
            .or_else(|| self.cause.as_ref().and_then(|cause| cause.api_error()))
    }
    pub fn login_error(&self) -> Option<&LoginError> {
        self.login
            .as_ref()
            .or_else(|| self.cause.as_ref().and_then(|cause| cause.login_error()))
    }
    pub fn service_account_login_error(&self) -> Option<&ServiceAccountLoginError> {
        self.service_login.as_ref().or_else(|| {
            self.cause
                .as_ref()
                .and_then(|cause| cause.service_account_login_error())
        })
    }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteDispatch {
    NotAWrite,
    NotSent,
    MayHaveBeenSent,
}

pub struct Failure<T> {
    error: Error,
    partial: Option<T>,
    dispatch: WriteDispatch,
}
impl<T> Failure<T> {
    #[allow(dead_code)]
    pub(crate) fn new(error: Error, partial: Option<T>, dispatch: WriteDispatch) -> Self {
        Self {
            error,
            partial,
            dispatch,
        }
    }
    #[allow(dead_code)]
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
    pub fn error(&self) -> &Error {
        &self.error
    }
    pub fn partial(&self) -> Option<&T> {
        self.partial.as_ref()
    }
    pub fn write_dispatch(&self) -> WriteDispatch {
        self.dispatch
    }
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
