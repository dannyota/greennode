use super::{ApiError, Error, ErrorClass, LoginError, SafeText, ServiceAccountLoginError};
use std::fmt;

#[derive(Clone, Debug, Default)]
pub(super) enum DisplayOverride {
    #[default]
    Original,
    Failed,
    Withheld,
    Empty,
}

impl DisplayOverride {
    fn screened(reflects: &impl Fn(&str) -> bool) -> Self {
        if !reflects("request failed") {
            Self::Failed
        } else if !reflects("response withheld") {
            Self::Withheld
        } else {
            Self::Empty
        }
    }

    fn text(&self) -> Option<&'static str> {
        match self {
            Self::Original => None,
            Self::Failed => Some("request failed"),
            Self::Withheld => Some("response withheld"),
            Self::Empty => Some(""),
        }
    }
}

#[derive(Debug)]
enum ServiceDisplay {
    Api,
    Login,
    ServiceAccountLogin,
    Text(SafeText),
    Joined(String),
    Withheld(DisplayOverride),
}

impl Error {
    pub(crate) fn with_service_display(self) -> Self {
        if self.service_display().is_some() {
            return self;
        }
        if self.api_error().is_some() {
            return self.with_outer_detail(ServiceDisplay::Api);
        }
        if self.login_error().is_some() {
            return self.with_outer_detail(ServiceDisplay::Login);
        }
        if self.service_account_login_error().is_some() {
            return self.with_outer_detail(ServiceDisplay::ServiceAccountLogin);
        }
        let text = self
            .detail::<String>()
            .map(String::as_str)
            .or(match self.class {
                ErrorClass::Cancelled => Some("context canceled"),
                ErrorClass::DeadlineExceeded => Some("context deadline exceeded"),
                ErrorClass::TokenCache => Some("vngcloud: token cache unavailable"),
                _ => None,
            });
        if let Some(text) = text {
            let safe = SafeText::new(text, &[]);
            if !safe.as_str().is_empty() {
                return self.with_outer_detail(ServiceDisplay::Text(safe));
            }
        }
        self
    }

    pub(crate) fn retain_status_visibility(mut self, source: Option<&ApiError>) -> Self {
        if let Some(source) = source.filter(|api| api.status_withheld) {
            let mut layer = Some(&mut self);
            while let Some(error) = layer {
                if let Some(api) = error.api.as_mut() {
                    if api.observed_status() == source.observed_status() {
                        api.status_withheld = true;
                        error.identity = true;
                    }
                    break;
                }
                layer = error.cause.0.as_deref_mut();
            }
        }
        self
    }

    fn service_display(&self) -> Option<&ServiceDisplay> {
        self.detail.as_ref()?.public()
    }

    pub(crate) fn with_local_diagnostic(self, text: &str) -> Self {
        self.with_outer_detail(ServiceDisplay::Text(SafeText::new(text, &[])))
    }

    /// Returns the screened diagnostic for a local error when one is available.
    pub fn local_diagnostic(&self) -> Option<&str> {
        match self.service_display()? {
            ServiceDisplay::Text(text) => Some(text.as_str()),
            ServiceDisplay::Withheld(display) => display.text(),
            _ => None,
        }
    }

    pub(crate) fn with_joined_service_display(self, text: String) -> Self {
        self.with_outer_detail(ServiceDisplay::Joined(text))
    }

    /// Returns the screened outer diagnostic for a failure that joins separate causes.
    pub fn joined_diagnostic(&self) -> Option<&str> {
        match self.service_display()? {
            ServiceDisplay::Joined(text) => Some(text),
            _ => None,
        }
    }

    pub(super) fn screen_service_display(&mut self, reflects: &impl Fn(&str) -> bool) {
        if self.service_display().is_some() && reflects(&self.to_string()) {
            let fallback = DisplayOverride::screened(reflects);
            let display = if self.joined_diagnostic().is_some() {
                ServiceDisplay::Joined(fallback.text().unwrap_or_default().to_owned())
            } else {
                ServiceDisplay::Withheld(fallback)
            };
            self.detail = Some(super::Detail::new(display));
        }
    }
}

impl ApiError {
    pub(super) fn screen_display(&mut self, reflects: &impl Fn(&str) -> bool) {
        if reflects(&self.to_string()) {
            self.display = DisplayOverride::screened(reflects);
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(text) = self.display.text() {
            return f.write_str(text);
        }
        if self.message_is_formatted && !self.message.is_empty() {
            return f.write_str(&self.message);
        }
        let operation = self.operation();
        if !operation.is_empty() {
            write!(f, "{operation}: ")?;
        }
        if self.message.is_empty() {
            f.write_str("request failed")?;
        } else {
            f.write_str(&self.message)?;
        }
        match self.response_status() {
            Some(status) => write!(f, " (status {status})"),
            None => match self.request_cause.as_deref() {
                Some(cause) if !cause.is_empty() => write!(f, ": {cause}"),
                _ => Ok(()),
            },
        }
    }
}

impl fmt::Display for LoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_login(f, self.reason(), self.status())?;
        if self.captcha_suspected() {
            f.write_str("; a captcha may be required")?;
        }
        Ok(())
    }
}

impl fmt::Display for ServiceAccountLoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_login(f, self.reason(), self.status())
    }
}

fn write_login(f: &mut fmt::Formatter<'_>, reason: &str, status: Option<u16>) -> fmt::Result {
    f.write_str(reason)?;
    if let Some(status) = status {
        write!(f, " (status {status})")?;
    }
    Ok(())
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.display_reflected {
            return f.write_str(self.diagnostic_fallback());
        }
        match self.service_display() {
            Some(ServiceDisplay::Text(text)) => return f.write_str(text.as_str()),
            Some(ServiceDisplay::Joined(text)) => return f.write_str(text),
            Some(ServiceDisplay::Withheld(display)) => {
                return f.write_str(display.text().unwrap_or_default());
            }
            Some(ServiceDisplay::Api) if let Some(api) = self.api_error() => return api.fmt(f),
            Some(ServiceDisplay::Login) if let Some(login) = self.login_error() => {
                return login.fmt(f);
            }
            Some(ServiceDisplay::ServiceAccountLogin)
                if let Some(login) = self.service_account_login_error() =>
            {
                return login.fmt(f);
            }
            _ => {}
        }
        write!(f, "greennode: {:?}", self.class)
    }
}
#[cfg(test)]
mod display_tests {
    use super::*;

    #[test]
    fn service_display_uses_typed_login_reason_and_status() {
        let iam = Error::new(ErrorClass::Auth)
            .with_login(LoginError::new("signin page", Some(503), true))
            .with_service_display();
        let account = Error::new(ErrorClass::Auth)
            .with_service_login(ServiceAccountLoginError::new("token exchange", Some(429)))
            .with_service_display();
        assert_eq!(
            iam.to_string(),
            "signin page (status 503); a captcha may be required"
        );
        assert_eq!(account.to_string(), "token exchange (status 429)");
    }
}
