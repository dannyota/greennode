use bytes::Bytes;
use http::{Method, StatusCode};
use serde::Serialize;
use zeroize::Zeroizing;

use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, RouteAuth},
    options::Options,
    redirect::{Classification, Retry},
    response, retry,
};
use crate::{ApiError, Error, ErrorClass, Failure, RecoveryAdvice, RequestContext, WriteDispatch};

use super::Client;
use super::wire::{invalid_input, screen_input};

pub(super) struct Write {
    pub(super) id: &'static str,
    pub(super) route: &'static str,
    pub(super) method: Method,
    pub(super) success: &'static [StatusCode],
    pub(super) sensitive: bool,
    pub(super) report_redirect: bool,
}

impl Client {
    #[expect(
        clippy::result_large_err,
        reason = "Keeps write dispatch in the public Failure type"
    )]
    pub(super) async fn send_once<O>(
        &self,
        ctx: &RequestContext,
        request: &Write,
        ids: &[&str],
        body: impl Serialize,
        evidence: &mut Evidence,
    ) -> Result<(StatusCode, Bytes), Failure<O>> {
        let session = &self.session;
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(ctx, &mut jitter)
            .map_err(|error| not_sent(*error))?;
        let project = session
            .require_project_id_with_evidence(ctx, evidence)
            .await
            .map_err(|error| not_sent(evidence.screen_error(*error)))?;
        let path: Vec<_> = std::iter::once(project.as_str())
            .chain(ids.iter().copied())
            .collect();
        screen_input(evidence, path.iter().copied()).map_err(|error| not_sent(*error))?;
        let base = session
            .endpoints()
            .vlb
            .as_ref()
            .ok_or_else(|| not_sent(Error::new(ErrorClass::InvalidConfig)))?;
        let mut encoded = Zeroizing::new(Vec::new());
        serde_json::to_writer(&mut *encoded, &body)
            .map_err(|_| not_sent(*invalid_input("cannot encode request")))?;
        let op = Operation {
            id: request.id,
            template: request.route,
            method: request.method.clone(),
            class: Classification::Write,
            retry: Retry::Once,
            success: request.success,
            cap: 8 << 20,
            allow_recovery: false,
            allow_empty: true,
            no_output: false,
            keep_status: false,
            overflow_overrides_status: true,
            sensitive: request.sensitive,
            reject_reflected_credentials: false,
            no_redirect: true,
            retry_config: retry::Config::default(),
        };
        let result = operation::execute_raw(
            runtime,
            base,
            &op,
            &Options {
                report_redirect: request.report_redirect,
                ..Options::DEFAULT
            },
            &path,
            &[],
            RouteAuth::Bearer(session.bearer_source()),
            Bytes::from_owner(encoded),
            evidence,
        )
        .await;
        let reply = result.map_err(|failure| {
            let (error, _, dispatch) = failure.into_parts();
            Failure::new(evidence.screen_error(error), None, dispatch)
        })?;
        Ok((reply.status, Bytes::from(reply.reply.body)))
    }
}

#[expect(
    clippy::result_large_err,
    reason = "Keeps write dispatch in the public Failure type"
)]
pub(super) fn reject<O>(
    request: &Write,
    status: StatusCode,
    body: Bytes,
    evidence: &Evidence,
    fixed_message: Option<&str>,
) -> Result<Bytes, Failure<O>> {
    response::classify(
        request.id,
        status,
        body,
        request.success,
        (true, false, Retry::Once),
        evidence.credentials(),
    )
    .map_err(|error| {
        let error = if let Some(message) = fixed_message {
            let code = error.api_error().map_or("", ApiError::code);
            api(request.id, Some(status), code, message, evidence)
        } else {
            evidence.screen_error(*error)
        };
        Failure::new(error, None, WriteDispatch::MayHaveBeenSent)
    })
}

pub(super) fn api(
    operation: &'static str,
    status: Option<StatusCode>,
    code: &str,
    message: &str,
    evidence: &Evidence,
) -> Error {
    use crate::error::SafeText;
    evidence.screen_error(Error::from_api(ApiError::without_fallback(
        operation,
        status.map(|status| status.as_u16()),
        SafeText::new(code, &[]),
        SafeText::new(message, &[]),
        false,
    )))
}

pub(super) fn advise<O>(failure: Failure<O>, message: &str, evidence: &Evidence) -> Failure<O> {
    let (error, _, dispatch) = failure.into_parts();
    if dispatch != WriteDispatch::MayHaveBeenSent
        || error
            .api_error()
            .and_then(ApiError::status)
            .is_some_and(|status| (400..500).contains(&status))
    {
        return Failure::new(error, None, dispatch);
    }
    let advice = response::sanitize_error_text(message, evidence.credentials());
    Failure::new(
        error.with_outer_detail(RecoveryAdvice::new(advice.as_str())),
        None,
        dispatch,
    )
}

pub(super) fn not_sent<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotSent)
}
