#[allow(dead_code)]
#[path = "../src/error.rs"]
mod error;

use error::{
    ApiError, Error, ErrorClass, Failure, LoginError, LoginFailureKind, SafeText,
    ServiceAccountLoginError, WriteDispatch,
};
use greennode_sdk::{ContextEnd, Secret, pricing};

fn safe(text: &str) -> SafeText {
    SafeText::new(text, &[])
}

#[test]
fn diagnostics_are_sanitized_before_access() {
    let credential = Secret::new("credential-canary");
    assert!(
        SafeText::new("prefix credential-canary suffix", &[&credential])
            .as_str()
            .is_empty()
    );
    assert!(
        SafeText::new("prefix credential-\u{202e}canary suffix", &[&credential])
            .as_str()
            .is_empty()
    );
    assert_eq!(
        SafeText::new("ok\n\u{202e}text", &[&credential]).as_str(),
        "oktext"
    );
    let api = ApiError::new(
        "test",
        None,
        SafeText::new("credential-canary", &[&credential]),
        SafeText::new("ok\ntext", &[&credential]),
        false,
    );
    assert!(api.code().is_empty());
    assert_eq!(api.message(), "oktext");
    let long = format!("{}é", "a".repeat(511));
    assert_eq!(SafeText::new(&long, &[]).as_str().len(), 511);
}

#[test]
fn fallback_handles_absent_status_and_preserves_explicit_codes() {
    let empty = ApiError::new("test", Some(404), safe(""), safe(""), false);
    assert_eq!(empty.code(), "NotFound");
    let null = ApiError::new("test", Some(404), safe("null"), safe(""), false);
    assert_eq!(null.code(), "NotFound");
    assert_eq!(
        ApiError::new("test", None, safe(""), safe(""), false).code(),
        ""
    );
    assert_eq!(
        ApiError::new("test", None, safe("null"), safe(""), false).code(),
        ""
    );
    let absent = ApiError::new("test", None, safe("0"), safe(""), false);
    assert_eq!(absent.code(), "");
    let status = ApiError::new("test", Some(404), safe("404"), safe(""), false);
    assert_eq!(status.code(), "NotFound");
    let custom = ApiError::new("test", Some(404), safe("Custom"), safe(""), false);
    assert_eq!(custom.code(), "Custom");
}

#[test]
fn login_details_survive_termination_and_causes() {
    let login = Error::from(ContextEnd::Cancelled).with_login(
        LoginError::new("login failed", Some(401), true)
            .with_kind(LoginFailureKind::SigninRejected),
    );
    let outer = Error::new(ErrorClass::Failed).with_cause(login);
    assert!(outer.has_identity(ErrorClass::Cancelled));
    assert!(!outer.has_identity(ErrorClass::Auth));
    assert_eq!(outer.login_error().map(LoginError::status), Some(Some(401)));
    assert_eq!(
        outer.login_error().map(LoginError::captcha_suspected),
        Some(true)
    );
    let account = Error::from(ContextEnd::DeadlineExceeded)
        .with_service_login(ServiceAccountLoginError::new("exchange failed", Some(503)));
    assert_eq!(
        account
            .service_account_login_error()
            .map(ServiceAccountLoginError::status),
        Some(Some(503))
    );
    assert!(!account.has_identity(ErrorClass::Auth));
}

#[test]
fn source_never_exposes_causes() {
    use std::error::Error as _;
    let error = Error::new(ErrorClass::Failed).with_cause(Error::from_api(ApiError::new(
        "test",
        Some(500),
        safe("code"),
        safe("message"),
        false,
    )));
    assert!(error.source().is_none());
    assert!(error.api_error().is_some());
}

#[test]
fn public_provider_failure_is_fixed() {
    let error = greennode_sdk::Error::provider_failed();
    assert_eq!(error.class(), greennode_sdk::ErrorClass::Auth);
    assert!(!error.to_string().is_empty());
    assert!(std::error::Error::source(&error).is_none());
}

#[test]
fn cancelled_context_has_cancelled_identity() {
    let error = Error::from(ContextEnd::Cancelled);
    assert_eq!(error.class(), ErrorClass::Cancelled);
    assert!(error.has_identity(ErrorClass::Cancelled));
    assert!(!error.has_identity(ErrorClass::DeadlineExceeded));
}

#[test]
fn expired_context_has_deadline_identity() {
    let error = Error::from(ContextEnd::DeadlineExceeded);
    assert_eq!(error.class(), ErrorClass::DeadlineExceeded);
    assert!(error.has_identity(ErrorClass::DeadlineExceeded));
    assert!(!error.has_identity(ErrorClass::Cancelled));
}

#[test]
fn first_status_and_attached_identity_are_distinct() {
    let inner = Error::from_api(ApiError::new("inner", Some(404), safe(""), safe(""), false));
    let outer = Error::from_api(ApiError::new("outer", Some(500), safe(""), safe(""), false))
        .with_cause(inner);
    assert!(!outer.has_class(ErrorClass::NotFound));
    assert!(!outer.has_identity(ErrorClass::NotFound));
    assert_eq!(outer.api_error().map(ApiError::operation), Some("outer"));
    let identified = outer.with_cause(Error::new(ErrorClass::NotFound));
    assert!(identified.has_identity(ErrorClass::NotFound));
    assert!(identified.has_class(ErrorClass::NotFound));
}

#[test]
fn dispatch_is_preserved_and_partial_is_hidden() {
    let inner = Failure::new(
        Error::new(ErrorClass::Failed),
        Some("partial-canary"),
        WriteDispatch::MayHaveBeenSent,
    );
    let outer = Failure::wrap(
        Error::new(ErrorClass::NotSettled),
        None::<()>,
        WriteDispatch::NotSent,
        inner,
    );
    assert_eq!(outer.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let shown = format!("{outer:?}");
    assert!(!shown.contains("partial-canary"));
    let partial = Failure::new(
        Error::new(ErrorClass::Failed),
        Some("partial-canary"),
        WriteDispatch::NotSent,
    );
    assert!(!format!("{partial:?}").contains("partial-canary"));
    assert_eq!(partial.partial(), Some(&"partial-canary"));
}

#[test]
fn api_message_never_formats() {
    let canary = "api-message-canary";
    let error = Error::from_api(ApiError::new(
        "operation",
        Some(500),
        safe("ServerError"),
        safe(canary),
        true,
    ));
    assert!(!format!("{error:?}").contains(canary));
    assert!(!format!("{error}").contains(canary));
    assert!(!format!("{:?}", error.api_error()).contains(canary));
    assert_eq!(error.api_error().map(ApiError::message), Some(canary));
}

#[test]
fn credentials_errors_include_invalid_config_identity() {
    for class in [ErrorClass::NoCredentials, ErrorClass::CredentialsFile] {
        let error = Error::new(class);
        assert!(error.has_identity(ErrorClass::InvalidConfig));
    }
    assert!(!Error::new(ErrorClass::InvalidInput).has_identity(ErrorClass::InvalidConfig));
}
