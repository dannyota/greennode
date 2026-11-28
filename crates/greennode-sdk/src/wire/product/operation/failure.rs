use super::super::{response, retry};
use super::{Operation, StatusCode, redirect, send, sender};

pub(super) fn invalid_response(operation: &Operation, status: StatusCode) -> crate::Error {
    use crate::error::SafeText;
    crate::Error::from_api(crate::ApiError::without_fallback(
        operation.id,
        operation.keep_status.then_some(status.as_u16()),
        SafeText::new("InvalidResponse", &[]),
        SafeText::new("response failed validation; body withheld", &[]),
        false,
    ))
    .with_http_status(status)
}

#[derive(Debug)]
pub(crate) struct BodyReadFailure;

#[derive(Debug)]
struct ActiveRequestInterruption;

pub(crate) fn is_active_transport_failure(error: &crate::Error) -> bool {
    error.detail::<ActiveRequestInterruption>().is_some()
        || error.detail::<BodyReadFailure>().is_some()
}

#[derive(Debug)]
pub(crate) struct DecodeFailure;

#[derive(Debug)]
struct RawOverflowStatus(StatusCode);

pub(crate) fn raw_overflow_status(error: &crate::Error) -> Option<StatusCode> {
    error.detail::<RawOverflowStatus>().map(|status| status.0)
}

pub(super) fn classify_failure(operation: &Operation, failure: sender::Failure) -> crate::Error {
    use crate::{ContextEnd, Error, ErrorClass};
    use sender::Error as SenderError;

    let status = failure.status;
    if let SenderError::Send(error) = &failure.error
        && matches!(error.kind(), send::Error::BodyTooLarge)
    {
        if operation.reflection == super::Reflection::Guard {
            let error = Error::bare_body_limit().with_outer_detail(super::RefusedGuard);
            return match status {
                Some(status) => error.with_http_status(status),
                None => error,
            };
        }
        if !operation.raw_body
            && !operation.overflow_overrides_status
            && let Some(status) = status.filter(|status| status.as_u16() >= 400)
        {
            let error = response::classify(
                operation.id,
                status,
                bytes::Bytes::new(),
                &[],
                (true, operation.sensitive, operation.retry),
                &failure.credentials,
            )
            .err()
            .map_or_else(|| Error::new(ErrorClass::Api), |error| *error)
            .with_outer_detail(status);
            return error;
        }
        let raw_status = status.filter(|status| {
            operation.raw_body
                && matches!(operation.class, redirect::Classification::Read)
                && !response::reflects_credential(status.as_str(), &failure.credentials)
        });
        let status = status.filter(|_| !operation.raw_body);
        let api = crate::ApiError::without_fallback(
            "",
            status.map(|status| status.as_u16()),
            crate::error::SafeText::new("", &[]),
            crate::error::SafeText::new("transport: response body exceeds limit", &[]),
            false,
        )
        .with_status_evidence();
        let error = Error::from_api(api).with_body_too_large();
        return match raw_status {
            Some(status) => error.with_outer_detail(RawOverflowStatus(status)),
            None => with_observed_status(error, status),
        };
    }

    let active_interruption = matches!(&failure.error, SenderError::ActiveRequestContext(_));
    let body_failure = matches!(&failure.error, SenderError::BodyContext(_))
        || matches!(&failure.error, SenderError::Send(error) if error.observed().is_some());
    let kept = status.filter(|_| operation.keep_status && body_failure);
    let (class, cause, retryable) = match failure.error {
        SenderError::Provider(error) => {
            return match status {
                Some(status) => error.with_http_status(status),
                None => *error,
            };
        }
        SenderError::Invalidation(error) => {
            if operation.keep_status && status == Some(StatusCode::UNAUTHORIZED) {
                return request_failure(
                    operation.id,
                    status,
                    FailureText::Cause("network request failed"),
                    false,
                    None,
                )
                .with_cause(*error);
            }
            return *error;
        }
        SenderError::InvalidRequest => return Error::new(ErrorClass::InvalidInput),
        SenderError::Send(error) if matches!(error.kind(), send::Error::InvalidRequest) => {
            return Error::new(ErrorClass::InvalidInput);
        }
        SenderError::Context(ContextEnd::Cancelled)
        | SenderError::ActiveRequestContext(ContextEnd::Cancelled)
        | SenderError::BodyContext(ContextEnd::Cancelled) => {
            (Some(ErrorClass::Cancelled), "canceled".into(), false)
        }
        SenderError::Context(ContextEnd::DeadlineExceeded)
        | SenderError::ActiveRequestContext(ContextEnd::DeadlineExceeded)
        | SenderError::BodyContext(ContextEnd::DeadlineExceeded) => (
            Some(ErrorClass::DeadlineExceeded),
            "timed out".into(),
            false,
        ),
        SenderError::Send(error) => match error.kind() {
            send::Error::Deadline => (
                Some(ErrorClass::DeadlineExceeded),
                "timed out".into(),
                false,
            ),
            send::Error::Transport(failure, cause) => (
                None,
                cause.message().into(),
                *failure == retry::SendFailure::FailedDial
                    || matches!(operation.retry, redirect::Retry::Repeatable),
            ),
            send::Error::BodyNetwork(cause) => (None, cause.message().into(), false),
            _ => (None, "network request failed".into(), false),
        },
        SenderError::Redirect(refusal) => (
            None,
            refusal.cause(),
            matches!(operation.retry, redirect::Retry::Repeatable),
        ),
    };
    let error = request_failure(
        operation.id,
        kept,
        FailureText::Cause(&cause),
        retryable,
        class,
    );
    let error = match status {
        Some(status) if kept.is_none() => error.with_observed_http_status(status),
        _ => error,
    };
    if body_failure {
        error.with_outer_detail(BodyReadFailure)
    } else if active_interruption {
        error.with_outer_detail(ActiveRequestInterruption)
    } else {
        error
    }
}

pub(super) enum FailureText<'a> {
    Message(&'a str),
    Cause(&'a str),
    MessageAndCause(&'a str, &'a str),
}

pub(super) fn request_failure(
    operation: &'static str,
    status: Option<StatusCode>,
    diagnostic: FailureText<'_>,
    retryable: bool,
    class: Option<crate::ErrorClass>,
) -> crate::Error {
    let (message, cause) = match diagnostic {
        FailureText::Message(message) => (message, None),
        FailureText::Cause(cause) => ("", Some(cause)),
        FailureText::MessageAndCause(message, cause) => (message, Some(cause)),
    };
    let mut api = crate::ApiError::new(
        operation,
        status.map(|status| status.as_u16()),
        crate::error::SafeText::new("", &[]),
        crate::error::SafeText::new(message, &[]),
        retryable,
    );
    if let Some(cause) = cause {
        api = api.with_request_cause(crate::error::SafeText::new(cause, &[]));
    }
    let api = crate::Error::from_api(api);
    let error = match class {
        Some(class) => crate::Error::new(class).with_cause(api),
        None => api,
    };
    with_observed_status(error, status)
}

fn with_observed_status(error: crate::Error, status: Option<StatusCode>) -> crate::Error {
    match status {
        Some(status) => error.with_outer_detail(status),
        None => error,
    }
}
