#[allow(dead_code)]
#[path = "../src/error.rs"]
mod error;

use error::{ApiError, Error, ErrorClass, Failure, WriteDispatch};
use greennode_sdk::ContextEnd;

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
    let inner = Error::from_api(ApiError::new("inner", Some(404), "", "", false));
    let outer = Error::from_api(ApiError::new("outer", Some(500), "", "", false)).with_cause(inner);
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
        "ServerError",
        canary,
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
