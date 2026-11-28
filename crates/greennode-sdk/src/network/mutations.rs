use std::time::Duration;

use bytes::Bytes;
use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::{Value, value::RawValue};

use crate::endpoint::Base;
use crate::error::SafeText;
use crate::model::wire::{Field, WireField, go_json};
use crate::wire::product::{
    evidence::{Evidence, SentCredentials},
    guard::{AcknowledgedId, Guarded},
    operation::{self, Operation, Reflection, RouteAuth},
    options::{AttemptCheck, Options, PathIds},
    output::PublicOutput,
    redirect::{Classification, Retry},
    retry,
    sender::Response,
};
use crate::{
    ApiError, ContextEnd, Error, ErrorClass, Failure, RecoveryAdvice, RequestContext, WriteDispatch,
};

use super::{Client, route_inventory::ids_reflect, wire::Fields};

pub(super) struct Mutation {
    pub(super) id: &'static str,
    pub(super) route: &'static str,
    pub(super) method: Method,
    pub(super) status: StatusCode,
    pub(super) success: &'static [StatusCode],
    pub(super) report_redirect: bool,
    retry: Retry,
    pub(super) keep_status: bool,
    pub(super) withhold: Option<&'static str>,
}

impl Mutation {
    pub(super) const fn new(
        id: &'static str,
        route: &'static str,
        method: Method,
        success: &'static [StatusCode],
    ) -> Self {
        let status = match success {
            [status, ..] => *status,
            [] => StatusCode::OK,
        };
        Self {
            id,
            route,
            method,
            status,
            success,
            report_redirect: false,
            retry: Retry::NonIdempotent,
            keep_status: false,
            withhold: None,
        }
    }

    pub(super) const fn repeatable(mut self) -> Self {
        self.retry = Retry::Repeatable;
        self
    }

    pub(super) const fn protected(mut self, keep_status: bool, message: &'static str) -> Self {
        self.report_redirect = true;
        self.retry = Retry::Once;
        self.keep_status = keep_status;
        self.withhold = Some(message);
        self
    }

    pub(super) fn invalid(&self, evidence: &Evidence) -> Error {
        api(
            self.id,
            None,
            "InvalidResponse",
            self.withhold.unwrap_or("invalid response; body withheld"),
            evidence,
        )
    }

    #[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
    pub(super) fn decode<T: WireField>(
        &self,
        response: &Response,
        evidence: &Evidence,
    ) -> Result<T, Error> {
        let receipt = response
            .clean_receipt()
            .ok_or_else(|| Error::new(ErrorClass::NotSettled).with_cause(self.invalid(evidence)))?;
        let text = go_json(receipt.body());
        if !text.is_empty() && serde_json::from_str::<&RawValue>(&text).is_err() {
            return Err(api(
                self.id,
                None,
                "",
                "response failed to decode; body withheld",
                evidence,
            ));
        }
        let invalid = |()| self.invalid(evidence);
        let fields = Fields::decode(if text.is_empty() || text == "null" {
            "{}"
        } else {
            &text
        })
        .map_err(invalid)?;
        strict_field(&fields, "data").map_err(invalid)
    }
}

pub(super) struct Destination<'a> {
    pub(super) base: &'a Base,
    pub(super) path: &'a [&'a str],
    pub(super) options: Options<'a>,
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Client {
    pub(super) async fn mutate_vserver<O>(
        &self,
        ctx: &RequestContext,
        write: &Mutation,
        ids: &[&str],
        body: Option<&Value>,
        evidence: &mut Evidence,
    ) -> Result<Response, Failure<O>> {
        check_input(write.id, evidence, &[], ids, body).map_err(not_sent)?;
        let project = self
            .session
            .require_project_id_with_evidence(ctx, evidence)
            .await
            .map_err(|error| not_sent(screen(*error, evidence)))?;
        let path: Vec<_> = std::iter::once(project.as_str())
            .chain(ids.iter().copied())
            .collect();
        let base = self.vserver_inventory_base().map_err(not_sent)?;
        self.mutate(
            ctx,
            write,
            Destination {
                base,
                path: &path,
                options: Options::DEFAULT,
            },
            body,
            evidence,
        )
        .await
    }

    pub(super) async fn mutate<O>(
        &self,
        ctx: &RequestContext,
        write: &Mutation,
        target: Destination<'_>,
        body: Option<&Value>,
        evidence: &mut Evidence,
    ) -> Result<Response, Failure<O>> {
        check_input(write.id, evidence, &[], target.path, body).map_err(not_sent)?;
        let encoded = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|_| not_sent(input_error(write.id, " could not encode the request")))?;
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|error| not_sent(*error))?;
        let op = Operation {
            id: write.id,
            template: write.route,
            method: write.method.clone(),
            class: Classification::Write,
            retry: write.retry,
            success: write.success,
            cap: if write.withhold.is_some() {
                4 << 20
            } else {
                8 << 20
            },
            allow_recovery: !matches!(write.retry, Retry::Once),
            allow_empty: true,
            no_output: false,
            keep_status: write.keep_status,
            raw_body: false,
            overflow_overrides_status: false,
            sensitive: write.withhold.is_some(),
            reflection: Reflection::Redact,
            no_redirect: matches!(write.retry, Retry::Once),
            report_redirect: write.report_redirect,
            retry_config: retry::Config::default(),
        };
        let attempt = |credentials: &SentCredentials| {
            check_input(
                write.id,
                &Evidence::from_credentials(credentials),
                &[],
                target.path,
                body,
            )
            .err()
            .map(Box::new)
            .or_else(|| {
                target
                    .options
                    .attempt_check
                    .and_then(|check| (check.0)(credentials))
            })
        };
        let options = Options {
            attempt_check: Some(AttemptCheck(&attempt)),
            ..target.options
        };
        let result = operation::execute_raw(
            runtime,
            target.base,
            &op,
            &options,
            target.path,
            &[],
            RouteAuth::Bearer(self.session.bearer_source()),
            encoded.map_or_else(Bytes::new, Bytes::from),
            evidence,
        )
        .await;
        let reply = result.map_err(|failure| {
            let (error, _, dispatch) = failure.into_parts();
            let error = match (write.withhold, error.api_error()) {
                (Some(message), Some(api))
                    if dispatch == WriteDispatch::MayHaveBeenSent
                        && api.is_for_operation(write.id)
                        && api
                            .observed_response_status()
                            .is_some_and(|status| status >= 300)
                        && !matches!(
                            error.class(),
                            ErrorClass::Cancelled | ErrorClass::DeadlineExceeded
                        ) =>
                {
                    with_write_message(write.id, error, message, evidence)
                }
                _ => screen(error, evidence),
            };
            Failure::new(error, None, dispatch)
        })?;
        reply
            .classify(
                write.id,
                op.success,
                (true, write.withhold.is_some(), write.retry),
                false,
            )
            .map_err(|error| {
                let error = match (write.withhold, error.api_error()) {
                    (Some(message), Some(_)) => {
                        with_write_message(write.id, *error, message, evidence)
                    }
                    _ => screen(*error, evidence),
                };
                sent(error)
            })?;
        reply
            .check_discarded_acknowledgement(write.id)
            .map_err(|error| sent(screen(*error, evidence)))?;
        Ok(reply)
    }
}

pub(super) fn with_write_message(
    op: &'static str,
    error: Error,
    message: &str,
    evidence: &Evidence,
) -> Error {
    let code = error.api_error().map_or("", ApiError::code);
    let code = SafeText::new(code, &[]);
    screen(
        error
            .with_api_operation(op)
            .with_api_diagnostic(code, SafeText::new(message, &[])),
        evidence,
    )
}

pub(super) fn api(
    id: &'static str,
    status: Option<u16>,
    code: &str,
    message: &str,
    evidence: &Evidence,
) -> Error {
    screen(
        Error::from_api(ApiError::without_fallback(
            id,
            status,
            SafeText::new(code, &[]),
            SafeText::new(message, &[]),
            false,
        )),
        evidence,
    )
}

pub(super) fn screen(error: Error, evidence: &Evidence) -> Error {
    evidence.screen_error(error)
}

pub(super) fn typed_error(
    class: ErrorClass,
    code: &str,
    message: &str,
    evidence: &Evidence,
) -> Error {
    Error::new(class).with_cause(api("", None, code, message, evidence))
}

pub(super) fn input_error(op: &str, reason: &str) -> Error {
    typed_error(
        ErrorClass::InvalidInput,
        "InvalidUsage",
        &format!("vngcloud: invalid input: {op}{reason}"),
        &Evidence::new(),
    )
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
pub(super) fn check_input(
    op: &str,
    evidence: &Evidence,
    fields: &[(&str, &str)],
    ids: &[&str],
    body: Option<&Value>,
) -> Result<(), Error> {
    if let Some((field, _)) = fields.iter().find(|(_, value)| value.is_empty()) {
        return Err(input_error(op, &format!(" requires {field}")));
    }
    if !evidence.can_retain_partial()
        || ids.iter().any(|id| evidence.id_reflects(id))
        || fields
            .iter()
            .any(|(_, value)| evidence.text_reflects(value))
        || body.is_some_and(|value| evidence.output_reflects(value))
    {
        return Err(input_error(op, " received a credential in the request"));
    }
    Ok(())
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
pub(super) fn path_id(op: &str, field: &str, id: &str) -> Result<(), Error> {
    if id.is_empty() {
        return Err(input_error(op, &format!(" requires {field}")));
    }
    if !PathIds::Strict.allows(id) {
        return Err(input_error(
            op,
            &format!(" requires {field} to match ^[A-Za-z0-9-]+$"),
        ));
    }
    Ok(())
}

pub(super) fn strict_field<T: WireField>(fields: &Fields<'_>, key: &str) -> Result<T, ()> {
    fields
        .raw(key)?
        .map(|raw| serde_json::from_str::<Field<T>>(raw.get()).map_err(|_| ()))
        .transpose()
        .map(|field| field.map_or_else(T::default, |field| field.0))
}

pub(super) fn retain<T: WireField + Serialize>(mut output: T, evidence: &Evidence) -> Option<T> {
    (evidence.can_retain_partial() && !ids_reflect(&output, evidence) && output.redact(evidence))
        .then_some(output)
}

pub(super) fn not_sent<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotSent)
}

pub(super) fn sent<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::MayHaveBeenSent)
}

pub(super) fn advise<T>(
    failure: Failure<T>,
    op: &str,
    action: &str,
    evidence: &Evidence,
) -> Failure<T> {
    let (error, partial, dispatch) = failure.into_parts();
    let advice = format!(
        "{op}: create may have already reached the server; {action}: {}",
        error_description(&error)
    );
    let error = if dispatch == WriteDispatch::MayHaveBeenSent
        && !error
            .api_error()
            .and_then(ApiError::observed_response_status)
            .is_some_and(|status| (400..500).contains(&status))
        && !evidence.text_reflects(&advice)
    {
        error.with_detail(RecoveryAdvice::new(&advice))
    } else {
        error
    };
    Failure::new(error, partial, dispatch)
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
pub(super) fn create_id(
    write: &Mutation,
    id: &str,
    polled: bool,
    message: &str,
    evidence: &Evidence,
) -> Result<(), Error> {
    if evidence.id_reflects(id) || (!id.is_empty() && !PathIds::WithUnderscore.allows(id)) {
        return Err(write.invalid(evidence));
    }
    if id.is_empty() || (polled && !PathIds::Strict.allows(id)) {
        return Err(api(
            write.id,
            Some(write.status.as_u16()),
            "",
            message,
            evidence,
        ));
    }
    Ok(())
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
pub(super) async fn tick(
    ctx: &RequestContext,
    clock: &dyn crate::Clock,
    interval: Duration,
) -> Result<(), Error> {
    let child = ctx.child_with_deadline(clock.monotonic_now() + interval);
    let _ = child.wait(clock).await;
    ctx.check(clock).map_or(Ok(()), |end| Err(end.into()))
}

pub(super) fn settlement<T>(
    op: &str,
    resource: &str,
    id: &str,
    cause: Error,
    partial: Option<T>,
    evidence: &Evidence,
) -> Failure<T> {
    let (class, code, message) = match cause.class() {
        ErrorClass::Failed => (
            ErrorClass::Failed,
            "WriteFailed",
            format!("vngcloud: write failed: {op}: {resource} {id} is ERROR"),
        ),
        ErrorClass::DeadlineExceeded if cause.detail::<ActiveWaitExpired>().is_some() => (
            ErrorClass::NotSettled,
            "NotSettled",
            format!(
                "vngcloud: write outcome not confirmed: {op}: {resource} {id} did not reach ACTIVE within 1m0s; the {resource} exists and this create must not be repeated"
            ),
        ),
        _ => {
            let reason = error_description(&cause);
            (
                ErrorClass::NotSettled,
                "NotSettled",
                format!("vngcloud: write outcome not confirmed: {op}: {resource} {id}: {reason}"),
            )
        }
    };
    let error = typed_error(class, code, &message, evidence).with_cause(cause);
    Failure::new(
        screen(error, evidence),
        partial,
        WriteDispatch::MayHaveBeenSent,
    )
}

pub(super) fn error_description(error: &Error) -> String {
    match error.class() {
        ErrorClass::Cancelled => return "context canceled".to_owned(),
        ErrorClass::DeadlineExceeded => return "context deadline exceeded".to_owned(),
        _ => {}
    }
    if let Some(api) = error.api_error() {
        let message = if api.message().is_empty() {
            "request failed"
        } else {
            api.message()
        };
        let mut full = if api.operation().is_empty() {
            message.to_owned()
        } else {
            format!("{}: {message}", api.operation())
        };
        if let Some(status) = api.status() {
            full.push_str(&format!(" (status {status})"));
        }
        return full;
    }
    error.to_string()
}

#[derive(Debug)]
struct ActiveWaitExpired;

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
pub(super) fn check_active_wait(
    wait: &RequestContext,
    parent: &RequestContext,
    clock: &dyn crate::Clock,
) -> Result<(), Error> {
    match wait.check(clock) {
        Some(ContextEnd::DeadlineExceeded) if parent.check(clock).is_none() => {
            Err(Error::new(ErrorClass::DeadlineExceeded).with_detail(ActiveWaitExpired))
        }
        Some(end) => Err(end.into()),
        None => Ok(()),
    }
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
pub(super) async fn tick_active_wait(
    wait: &RequestContext,
    parent: &RequestContext,
    clock: &dyn crate::Clock,
) -> Result<(), Error> {
    let tick = wait.child_with_deadline(clock.monotonic_now() + Duration::from_secs(2));
    let _ = tick.wait(clock).await;
    check_active_wait(wait, parent, clock)
}

pub(super) fn safe_output<T: Serialize>(output: &T, evidence: &Evidence) -> bool {
    evidence.can_retain_partial()
        && !super::route_inventory::ids_reflect(output, evidence)
        && serde_json::to_value(output).is_ok_and(|value| !evidence.output_reflects(&value))
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
pub(super) fn check_quote(
    op: &'static str,
    kind: &str,
    price: f64,
    cap: f64,
    evidence: &mut Evidence,
    amounts: &[f64],
) -> Result<(), Error> {
    evidence.recheck_read_proofs().map_err(|error| *error)?;
    let (price, cap) = Guarded::price(evidence, price, cap)
        .and_then(|guarded| guarded.into_checked(evidence))
        .map_err(|error| *error)?;
    let credentials = evidence.credentials();
    if !evidence.can_retain_partial()
        || evidence.text_reflects("VND")
        || amounts.iter().chain([&cap]).any(|value| {
            crate::output_number::reflects(*value, credentials, true)
                || evidence.text_reflects(&format!("{value:.0}"))
        })
    {
        return Err(api(
            op,
            Some(200),
            "InvalidResponse",
            "price response withheld",
            evidence,
        ));
    }
    if price > cap {
        let message = format!(
            "vngcloud: quoted price is above MaxPrice: {op}: {kind} {price:.0} VND exceeds MaxPrice {cap:.0} VND"
        );
        return Err(typed_error(
            ErrorClass::PriceAboveMax,
            "PriceAboveMax",
            &message,
            evidence,
        ));
    }
    Ok(())
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
pub(super) fn check_context(ctx: &RequestContext, clock: &dyn crate::Clock) -> Result<(), Error> {
    ctx.check(clock).map_or(Ok(()), |end| Err(end.into()))
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
pub(super) fn bind_acknowledgement(
    mutation: &Mutation,
    reply: &Response,
    id: &str,
    evidence: &mut Evidence,
) -> Result<AcknowledgedId, Error> {
    let receipt = reply
        .clean_receipt()
        .ok_or_else(|| mutation.invalid(evidence))?;
    let id = receipt.bind_id(id.to_owned());
    evidence.keep_acknowledgement(&id).map_err(|error| *error)?;
    Ok(id)
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
pub(super) fn guard_operands<T: PublicOutput>(
    op: &'static str,
    value: T,
    evidence: &mut Evidence,
) -> Result<T, Error> {
    if evidence.guards_reads() {
        evidence.recheck_read_proofs().map_err(|error| *error)?;
        evidence
            .keep_decoded_operands(op, &value)
            .map_err(|error| *error)?;
        return Guarded::latest_direct(value, evidence, op)
            .and_then(|proof| proof.into_checked(evidence))
            .map_err(|error| *error);
    }
    Ok(value)
}
