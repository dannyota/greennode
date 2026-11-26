use super::{Client, Instance};
use crate::config::session::AuthKind;
use crate::endpoint::Base;
use crate::error::SafeText;
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, Output},
    options::Options,
    redirect::{Classification, Retry},
    retry,
};
use crate::{ApiError, Error, ErrorClass, Failure, RequestContext, WriteDispatch};
use bytes::Bytes;
use http::{Method, StatusCode};
use serde_json::Value;
use std::{task::Poll, time::Duration};

const POLL_INTERVAL: Duration = Duration::from_secs(10);
const POLL_BOUND: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Copy)]
pub(super) enum Settlement {
    NotSettled(&'static str),
    Failed,
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    pub(super) fn write_provenance<O>(&self) -> Result<(), Failure<O>> {
        let current = self.session.endpoints();
        let default = self.session.default_endpoints();
        let same_base = |current: &Option<Base>, default: &Option<Base>| {
            current.as_ref().map(Base::configured_url) == default.as_ref().map(Base::configured_url)
        };
        let trusted = self.session.auth_kind() == AuthKind::IamUser
            && self.session.uses_default_auth_endpoints()
            && same_base(&current.database, &default.database);
        #[cfg(feature = "test-support")]
        let trusted = trusted
            || (self.session.is_database_write_fixture()
                && self.session.auth_kind() == AuthKind::Static);
        if self.supported_region && trusted && same_base(&current.vserver, &default.vserver) {
            return Ok(());
        }
        Err(not_sent(local_error(
            ErrorClass::InvalidConfig,
            "InvalidConfig",
            "vngcloud: invalid config: database writes require hcm-3, SDK IAM-user login, and default endpoints",
        )))
    }

    pub(super) async fn send_write<T: Output>(
        &self,
        ctx: &RequestContext,
        operation: &Operation,
        options: &Options,
        path: &[&str],
        body: Bytes,
        evidence: &mut Evidence,
    ) -> Result<T, Failure<T>> {
        let session = &self.session;
        let base = session
            .endpoints()
            .database
            .as_ref()
            .ok_or_else(|| not_sent(Error::new(ErrorClass::InvalidConfig)))?;
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(ctx, &mut jitter)
            .map_err(|error| not_sent(*error))?;
        operation::execute_with_evidence(
            runtime,
            base,
            operation,
            options,
            path,
            &[],
            session.bearer_source(),
            body,
            evidence,
        )
        .await
        .map(|(_, output)| output)
    }

    pub(super) async fn poll_instance<T>(
        &self,
        context: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
        interrupted: Settlement,
        mut check: impl FnMut(Option<Instance>) -> Poll<Result<T, Settlement>>,
    ) -> Result<T, Settlement> {
        let clock = self.session.clock();
        let deadline = clock.monotonic_now() + POLL_BOUND;
        let ctx = context.child_with_deadline(deadline);
        while ctx.check(clock).is_none() {
            let instance = match self.get_instance::<()>(&ctx, id, evidence).await {
                Ok(instance) => Some(instance),
                Err(failure) if failure.error().has_class(ErrorClass::NotFound) => None,
                Err(_) => break,
            };
            if ctx.check(clock).is_some() {
                break;
            }
            if let Poll::Ready(result) = check(instance) {
                return result;
            }
            let wake = clock.monotonic_now() + POLL_INTERVAL;
            ctx.child_with_deadline(wake).wait(clock).await;
        }
        Err(interrupted)
    }
}

pub(super) fn local_error(class: ErrorClass, code: &str, message: &str) -> Error {
    Error::new(class).with_cause(api_error("", None, code, message, false))
}

pub(super) fn api_error(
    operation: &'static str,
    status: Option<u16>,
    code: &str,
    message: &str,
    retryable: bool,
) -> Error {
    Error::from_api(ApiError::new(
        operation,
        status,
        SafeText::new(code, &[]),
        SafeText::new(message, &[]),
        retryable,
    ))
}

pub(super) fn not_sent<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotSent)
}

pub(super) fn preflight<T, O>(failure: Failure<T>, operation: &'static str) -> Failure<O> {
    let (error, _, _) = failure.into_parts();
    if let Some(api) = error.api_error()
        && api.code() == "InvalidResponse"
    {
        return not_sent(api_error(
            operation,
            api.status(),
            api.code(),
            api.message(),
            api.retryable(),
        ));
    }
    not_sent(safe_rejection(
        operation,
        &error,
        "request failed; response withheld",
    ))
}

pub(super) fn safe_rejection(operation: &'static str, error: &Error, message: &str) -> Error {
    let api = error.api_error();
    let status = api.and_then(ApiError::status);
    let retryable = api.is_some_and(ApiError::retryable);
    let safe = api_error(operation, status, "", message, retryable);
    for class in [
        ErrorClass::Cancelled,
        ErrorClass::DeadlineExceeded,
        ErrorClass::Auth,
        ErrorClass::PermissionDenied,
        ErrorClass::NotFound,
        ErrorClass::RateLimited,
    ] {
        if error.has_class(class) {
            return safe.with_cause(Error::new(class));
        }
    }
    safe
}

pub(super) fn rejection<T, O>(failure: Failure<T>, operation: &'static str) -> Option<Failure<O>> {
    let (error, _, dispatch) = failure.into_parts();
    let api_status = error.api_error().and_then(ApiError::status);
    let status = api_status.or_else(|| error.detail::<StatusCode>().map(StatusCode::as_u16));
    if dispatch != WriteDispatch::NotSent
        && !status.is_some_and(|status| (400..500).contains(&status))
    {
        return None;
    }
    let message = match error.detail::<super::create::RejectedFields>() {
        Some(fields) if !fields.0.is_empty() => {
            format!("invalid input rejected by the API: {}", fields.0.join(", "))
        }
        _ if api_status.is_some_and(|status| status >= 400) => {
            "request rejected by the API".to_owned()
        }
        _ => "request failed; response withheld".to_owned(),
    };
    Some(Failure::new(
        safe_rejection(operation, &error, &message),
        None,
        dispatch,
    ))
}

pub(super) fn unsettled<O>(
    operation: &'static str,
    settlement: Settlement,
    id: Option<String>,
    evidence: &Evidence,
    partial: impl FnOnce(String) -> O,
) -> Failure<O> {
    let (class, reason) = match settlement {
        Settlement::NotSettled(reason) => (ErrorClass::NotSettled, reason),
        Settlement::Failed => (ErrorClass::Failed, "vngcloud: write failed"),
    };
    let message =
        format!("{operation}: inspect inventory and billing before another mutation: {reason}");
    let error = local_error(class, "", &message);
    let dispatch = WriteDispatch::MayHaveBeenSent;
    let Some(id) = id else {
        return Failure::new(error, None, dispatch);
    };
    let failure = evidence.with_partial(
        Failure::<()>::new(error, None, dispatch),
        Value::String(id),
        dispatch,
    );
    let (error, value, dispatch) = failure.into_parts();
    Failure::new(
        error,
        value.and_then(|value| match value {
            Value::String(id) => Some(partial(id)),
            _ => None,
        }),
        dispatch,
    )
}

pub(super) fn write_operation(
    id: &'static str,
    template: &'static str,
    success: &'static [StatusCode],
    allow_empty: bool,
) -> Operation {
    Operation {
        id,
        template,
        method: Method::POST,
        class: Classification::Write,
        retry: Retry::Once,
        success,
        cap: 1 << 20,
        allow_recovery: false,
        allow_empty,
        no_output: false,
        keep_status: true,
        overflow_overrides_status: false,
        sensitive: true,
        reject_reflected_credentials: true,
        no_redirect: true,
        retry_config: retry::Config::default(),
    }
}
