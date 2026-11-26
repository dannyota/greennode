use crate::{ApiError, Error, ErrorClass, LoginError, error::SafeText};

pub fn identity(class: ErrorClass) -> Error {
    Error::new(class)
}

pub fn api(operation: &'static str, status: Option<u16>, code: &str, message: &str) -> Error {
    Error::from_api(ApiError::new(
        operation,
        status,
        SafeText::new(code, &[]),
        SafeText::new(message, &[]),
        false,
    ))
}

pub fn login(reason: &'static str, status: Option<u16>) -> Error {
    Error::new(ErrorClass::Auth).with_login(LoginError::new(reason, status, false))
}

pub fn cause(error: Error, child: Error) -> Error {
    error.with_cause(child)
}
