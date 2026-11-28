use std::net::IpAddr;
use std::time::Duration;

use bytes::Bytes;
use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::{Value, value::RawValue};

use crate::error::SafeText;
use crate::model::wire::{Field, WireField, go_json};
use crate::wire::product::{
    evidence::Evidence,
    guard::AcknowledgedId,
    operation::{self, Operation, Reflection, RouteAuth},
    options::{AttemptCheck, Options, PathIds},
    redirect::{Classification, Retry},
    response, retry,
    sender::Response,
};
use crate::{ApiError, Error, ErrorClass, Failure, RecoveryAdvice, RequestContext, WriteDispatch};

use super::super::super::{Client, wire::Fields};
use super::super::wire::Reads;

pub(super) const BOUND: Duration = Duration::from_secs(60);
pub(super) const CREATE_BOUND: Duration = Duration::from_secs(180);
pub(super) const INTERVAL: Duration = Duration::from_secs(2);

/// Network write refusal reported as service-specific error detail.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum NetworkWriteError {
    /// The resource is managed by the service.
    DefaultResource,
    /// The resource is busy or changed between guard reads.
    Busy,
    /// The provider reported a state the operation cannot handle.
    UnexpectedStatus,
}

pub(super) struct Write<'a> {
    pub(super) client: &'a Client,
    pub(super) ctx: &'a RequestContext,
    pub(super) reads: Reads<'a>,
    acknowledged: Option<AcknowledgedId>,
}

#[derive(Clone, Copy)]
pub(super) struct Request {
    pub(super) id: &'static str,
    template: &'static str,
    method: &'static Method,
    success: &'static [StatusCode],
    retry: Retry,
    withheld: Option<&'static str>,
}

impl Request {
    pub(super) const fn new(
        id: &'static str,
        template: &'static str,
        method: &'static Method,
        success: &'static [StatusCode],
    ) -> Self {
        Self {
            id,
            template,
            method,
            success,
            retry: Retry::NonIdempotent,
            withheld: None,
        }
    }

    pub(super) const fn once(mut self) -> Self {
        self.retry = Retry::Once;
        self
    }

    pub(super) const fn repeatable(mut self) -> Self {
        self.retry = Retry::Repeatable;
        self
    }

    pub(super) const fn withheld(mut self, message: &'static str) -> Self {
        self.withheld = Some(message);
        self
    }
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl<'a> Write<'a> {
    pub(super) fn new<O>(
        client: &'a Client,
        ctx: &'a RequestContext,
        evidence: Evidence,
    ) -> Result<Self, Failure<O>> {
        Ok(Self {
            client,
            ctx,
            reads: Reads::with_evidence(
                client,
                ctx,
                client
                    .request_evidence_from(ctx, evidence)
                    .map_err(not_sent)?,
            ),
            acknowledged: None,
        })
    }

    pub(super) fn evidence(&self) -> &Evidence {
        self.reads.evidence()
    }

    pub(super) fn check_proofs(&self) -> Result<(), Error> {
        if let Some(id) = &self.acknowledged {
            id.recheck(self.evidence()).map_err(|error| *error)?;
        }
        self.evidence()
            .recheck_read_proofs()
            .map_err(|error| *error)
    }

    pub(super) fn screen(&self, error: Error) -> Error {
        self.reads.screen(error)
    }

    pub(super) fn local(&self, class: ErrorClass, message: &str) -> Error {
        let code = if class == ErrorClass::InvalidInput {
            "InvalidUsage"
        } else {
            ""
        };
        self.screen(Error::new(class).with_cause(api("", None, code, message)))
    }

    pub(super) fn guard_error(&self, detail: NetworkWriteError, message: &str) -> Error {
        let label = match detail {
            NetworkWriteError::DefaultResource => "default resource",
            NetworkWriteError::Busy => "resource busy",
            NetworkWriteError::UnexpectedStatus => "unexpected status",
        };
        self.local(ErrorClass::Api, &format!("network: {label}: {message}"))
            .with_outer_detail(detail)
    }

    pub(super) fn in_use(&self, message: &str) -> Error {
        let message = format!("network: resource in use: {message}");
        self.screen(api("", None, "ResourceInUse", &message))
            .with_outer_detail(crate::network::ResourceInUse)
    }

    pub(super) fn invalid(&self, op: &'static str) -> Error {
        self.screen(api(
            op,
            None,
            "InvalidResponse",
            "invalid or incomplete inventory; body withheld",
        ))
    }

    pub(super) fn check<O>(
        &self,
        op: &'static str,
        fields: &[(&str, &str)],
        path: &[(&str, &str)],
    ) -> Result<(), Failure<O>> {
        for &(field, value) in path.iter().chain(fields) {
            required(op, field, !value.is_empty())?;
        }
        for &(field, value) in path {
            if !PathIds::Strict.allows(value) {
                return Err(input(
                    op,
                    &format!("requires {field} to match ^[A-Za-z0-9-]+$"),
                ));
            }
        }
        for &(field, value) in fields {
            if self.evidence().text_reflects(value) {
                return Err(input(op, &format!("requires {field} without credentials")));
            }
        }
        for &(field, value) in path {
            if self.evidence().id_reflects(value) {
                return Err(input(op, &format!("requires {field} without credentials")));
            }
        }
        Ok(())
    }

    pub(super) fn safe<T: Serialize>(&self, op: &'static str, row: &T) -> Result<(), Error> {
        screen_value(row, self.evidence()).map_err(|()| self.invalid(op))
    }

    pub(super) fn scoped(&self, bound: Duration) -> RequestContext {
        self.ctx
            .child_with_deadline(self.client.session.clock().monotonic_now() + bound)
    }

    pub(super) fn ended(&self, ctx: &RequestContext) -> bool {
        ctx.check(self.client.session.clock()).is_some()
    }

    pub(super) async fn tick(&self, ctx: &RequestContext, interval: Duration) {
        let clock = self.client.session.clock();
        ctx.child_with_deadline(clock.monotonic_now() + interval)
            .wait(clock)
            .await;
    }

    pub(super) async fn send<O>(
        &mut self,
        request: Request,
        ids: &[&str],
        body: Option<Value>,
    ) -> Result<Response, Failure<O>> {
        let check = |evidence: &Evidence| {
            if ids.iter().any(|id| evidence.id_reflects(id))
                || body
                    .as_ref()
                    .is_some_and(|value| evidence.output_reflects(value))
            {
                Err(input(
                    request.id,
                    "requires request values without credentials",
                ))
            } else {
                Ok(())
            }
        };
        check(self.evidence())?;
        let session = &self.client.session;
        let project = session
            .require_project_id_with_evidence(self.ctx, self.reads.evidence_mut())
            .await
            .map_err(|err| not_sent(self.screen(*err)))?;
        check(self.evidence())?;
        self.check_proofs().map_err(not_sent)?;
        if self.evidence().id_reflects(&project) {
            return Err(input(
                request.id,
                "requires a project identifier without credentials",
            ));
        }
        let path: Vec<_> = std::iter::once(project.as_str())
            .chain(ids.iter().copied())
            .collect();
        let encoded = body
            .as_ref()
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|_| input(request.id, "requires an encodable request body"))?
            .unwrap_or_default();
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(self.ctx, &mut jitter)
            .map_err(|err| not_sent(self.screen(*err)))?;
        let operation = Operation {
            id: request.id,
            template: request.template,
            method: request.method.clone(),
            class: Classification::Write,
            retry: request.retry,
            success: request.success,
            cap: 8 << 20,
            allow_recovery: !matches!(request.retry, Retry::Once),
            allow_empty: true,
            no_output: false,
            keep_status: false,
            raw_body: false,
            overflow_overrides_status: false,
            sensitive: request.withheld.is_some(),
            reflection: Reflection::Redact,
            no_redirect: matches!(request.retry, Retry::Once),
            report_redirect: matches!(request.retry, Retry::Once),
            retry_config: retry::Config::default(),
        };
        let attempt = |credentials: &crate::wire::product::evidence::SentCredentials| {
            let mut evidence = Evidence::new();
            evidence.absorb(credentials);
            check(&evidence)
                .err()
                .map(|failure: Failure<O>| Box::new(failure.into_parts().0))
        };
        let options = Options {
            attempt_check: Some(AttemptCheck(&attempt)),
            ..Options::DEFAULT
        };
        let result = operation::execute_raw(
            runtime,
            self.client.vserver_inventory_base().map_err(not_sent)?,
            &operation,
            &options,
            &path,
            &[],
            RouteAuth::Bearer(session.bearer_source()),
            Bytes::from(encoded),
            self.reads.evidence_mut(),
        )
        .await;
        let reply = result.map_err(|error| {
            let (mut error, _, dispatch) = error.into_parts();
            if error.http_status().is_some()
                && matches!(
                    error.class(),
                    ErrorClass::Cancelled | ErrorClass::DeadlineExceeded
                )
            {
                error = error.with_outer_detail(InterruptedResponse);
            }
            Failure::new(self.screen(withhold(error, request)), None, dispatch)
        })?;
        reply
            .classify(
                request.id,
                request.success,
                (true, request.withheld.is_some(), operation.retry),
                false,
            )
            .map_err(|error| sent(self.screen(withhold(*error, request)), None))?;
        reply
            .check_discarded_acknowledgement(request.id)
            .map_err(|error| sent(self.screen(*error), None))?;
        Ok(reply)
    }

    pub(super) fn created<T: WireField + Serialize, O>(
        &mut self,
        request: Request,
        response: Response,
        data: bool,
        identity: fn(&T) -> &str,
        path_id: bool,
        advice: &str,
    ) -> Result<T, Failure<O>> {
        self.created_with(
            request,
            response,
            (data, path_id),
            identity,
            advice,
            |row| {
                serde_json::from_str::<Field<T>>(row)
                    .map(|field| field.0)
                    .map_err(|_| ())
            },
        )
    }

    pub(super) fn created_with<T: WireField + Serialize, O>(
        &mut self,
        request: Request,
        response: Response,
        (data, path_id): (bool, bool),
        identity: fn(&T) -> &str,
        advice: &str,
        parse: fn(&str) -> Result<T, ()>,
    ) -> Result<T, Failure<O>> {
        let status = response.status;
        let receipt = response.clean_receipt().ok_or_else(|| {
            sent(
                Error::new(ErrorClass::NotSettled).with_cause(self.invalid(request.id)),
                None,
            )
        })?;
        let text = go_json(receipt.body());
        let text = if text.is_empty() { "null" } else { &text };
        let invalid = || sent(self.invalid(request.id), None);
        serde_json::from_str::<&RawValue>(text).map_err(|_| {
            self.advise(
                sent(
                    self.screen(api(request.id, None, "", "response failed to decode")),
                    None,
                ),
                advice,
            )
        })?;
        crate::wire::strict_json::validate(text.as_bytes(), &[], text.len())
            .map_err(|_| invalid())?;
        let row = if data && text != "null" {
            let fields: Fields<'_> = serde_json::from_str(text).map_err(|_| invalid())?;
            fields
                .raw("data")
                .map_err(|()| invalid())?
                .map_or("null", RawValue::get)
        } else {
            text
        };
        let mut out = parse(row).map_err(|()| invalid())?;
        let id = identity(&out);
        if id.is_empty() {
            let message = format!("create response had no id; {advice}");
            return Err(sent(
                self.screen(api(request.id, Some(status.as_u16()), "", &message)),
                None,
            ));
        }
        let allowed = if path_id {
            PathIds::Strict
        } else {
            PathIds::WithUnderscore
        };
        if !allowed.allows(id) || self.evidence().id_reflects(id) {
            return Err(invalid());
        }
        let acknowledged = receipt.bind_id(id.to_owned());
        if !out.redact(self.evidence()) {
            return Err(invalid());
        }
        self.safe(request.id, &out)
            .map_err(|error| sent(error, None))?;
        self.reads
            .evidence_mut()
            .keep_acknowledgement(&acknowledged)
            .map_err(|error| sent(*error, None))?;
        self.acknowledged = Some(acknowledged);
        Ok(out)
    }

    pub(super) fn advise<O>(&self, failure: Failure<O>, advice: &str) -> Failure<O> {
        let (mut error, partial, dispatch) = failure.into_parts();
        if dispatch == WriteDispatch::MayHaveBeenSent && !is_4xx(&error) {
            let reason = error_text(&error);
            let operation = error.api_error().map_or("", ApiError::operation);
            let action = advice.split_once(", ").map_or(advice, |(_, action)| action);
            let full = if advice.starts_with("the change") {
                format!("{operation}: {advice}: {reason}")
            } else {
                format!(
                    "{operation}: create may have already reached the server; {action}: {reason}"
                )
            };
            let advice = response::sanitize_error_text(&full, self.evidence().credentials());
            error = error.with_outer_detail(RecoveryAdvice::new(advice.as_str()));
        }
        Failure::new(error, partial, dispatch)
    }

    pub(super) fn outcome<T, O: Serialize>(
        &self,
        op: &'static str,
        state: T,
        result: Result<(), Error>,
        build: impl FnOnce(T) -> O,
    ) -> Result<O, Failure<O>> {
        let output = build(state);
        let checked = self.check_proofs().and_then(|()| self.safe(op, &output));
        match result {
            Ok(()) => checked.map(|()| output).map_err(|error| sent(error, None)),
            Err(error) => Err(sent(
                self.screen(error),
                (self.evidence().can_retain_partial() && checked.is_ok()).then_some(output),
            )),
        }
    }

    pub(super) fn map_result<T, O: Serialize>(
        &self,
        op: &'static str,
        result: Result<T, Failure<T>>,
        output: impl FnOnce(T) -> O,
    ) -> Result<O, Failure<O>> {
        match result {
            Ok(value) => self.outcome(op, value, Ok(()), output),
            Err(failure) => {
                let (error, partial, dispatch) = failure.into_parts();
                let partial = partial
                    .map(output)
                    .filter(|value| self.safe(op, value).is_ok());
                Err(Failure::new(error, partial, dispatch))
            }
        }
    }

    pub(super) fn unchanged<O: Serialize>(
        &self,
        op: &'static str,
        output: O,
    ) -> Result<O, Failure<O>> {
        self.check_proofs().map_err(not_sent)?;
        self.safe(op, &output).map_err(not_sent)?;
        Ok(output)
    }

    pub(super) fn settled_error(&self, message: &str, cause: Error) -> Error {
        self.settled_error_with_confirmation(message, cause, None)
    }

    pub(super) fn settled_error_with_confirmation(
        &self,
        message: &str,
        cause: Error,
        confirmation: Option<Error>,
    ) -> Error {
        let mut message = format!(
            "vngcloud: write outcome not confirmed: {message}: {}",
            error_text(&cause)
        );
        let cause = if let Some(error) = confirmation {
            let reason = error
                .detail::<RecoveryAdvice>()
                .map_or_else(|| error_text(&error), RecoveryAdvice::message);
            message.push_str("; ");
            message.push_str(reason);
            cause.with_cause(error)
        } else {
            cause
        };
        let message = response::sanitize_error_text(&message, self.evidence().credentials());
        self.screen(Error::new(ErrorClass::NotSettled).with_cause(cause))
            .with_outer_detail(RecoveryAdvice::new(message.as_str()))
    }
}

pub(super) fn screen_value<T: Serialize>(row: &T, evidence: &Evidence) -> Result<(), ()> {
    let value = serde_json::to_value(row).map_err(|_| ())?;
    if evidence.output_reflects(&value) {
        return Err(());
    }
    Ok(())
}

fn error_text(error: &Error) -> &str {
    if let Some(message) = error
        .api_error()
        .map(ApiError::message)
        .filter(|value| !value.is_empty())
    {
        return message;
    }
    match error.class() {
        ErrorClass::Cancelled => "canceled",
        ErrorClass::DeadlineExceeded => "timed out",
        _ => "request failed",
    }
}

fn withhold(error: Error, request: Request) -> Error {
    if let Some(message) = request.withheld
        && let Some(api_error) = error.api_error()
        && api_error.is_for_operation(request.id)
        && api_error.observed_response_status().is_some()
    {
        let code = SafeText::new(api_error.code(), &[]);
        return error.with_api_diagnostic(code, SafeText::new(message, &[]));
    }
    error
}

pub(super) fn api(op: &'static str, status: Option<u16>, code: &str, message: &str) -> Error {
    Error::from_api(ApiError::without_fallback(
        op,
        status,
        SafeText::new(code, &[]),
        SafeText::new(message, &[]),
        false,
    ))
}

#[derive(Debug)]
struct InterruptedResponse;

pub(super) fn can_confirm(error: &Error) -> bool {
    error.detail::<InterruptedResponse>().is_none() && !error.is_body_too_large()
}

pub(super) fn is_4xx(error: &Error) -> bool {
    error
        .api_error()
        .and_then(ApiError::observed_response_status)
        .is_some_and(|status| (400..500).contains(&status))
}

pub(super) fn is_5xx(error: &Error) -> bool {
    error
        .api_error()
        .and_then(ApiError::observed_response_status)
        .is_some_and(|status| status >= 500)
}

pub(super) fn not_sent<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotSent)
}

pub(super) fn sent<T>(error: Error, partial: Option<T>) -> Failure<T> {
    Failure::new(error, partial, WriteDispatch::MayHaveBeenSent)
}

pub(super) fn input<T>(op: &'static str, message: &str) -> Failure<T> {
    not_sent(Error::new(ErrorClass::InvalidInput).with_cause(api(
        "",
        None,
        "InvalidUsage",
        &format!("vngcloud: invalid input: {op} {message}"),
    )))
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
pub(super) fn required<T>(op: &'static str, field: &str, present: bool) -> Result<(), Failure<T>> {
    if present {
        Ok(())
    } else {
        Err(input(op, &format!("requires {field}")))
    }
}

pub(super) fn prefix(value: &str) -> Option<(IpAddr, u8, bool)> {
    let (address, bits) = value.split_once('/')?;
    if !bits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if bits.len() > 1 && bits.starts_with('0') {
        return None;
    }
    let address: IpAddr = address.parse().ok()?;
    let bits: u8 = bits.parse().ok()?;
    let (number, width) = match address {
        IpAddr::V4(value) => (u128::from(u32::from(value)), 32u8),
        IpAddr::V6(value) => (u128::from(value), 128),
    };
    let host_bits = width.checked_sub(bits)?;
    let mask = 1u128
        .checked_shl(host_bits.into())
        .map_or(u128::MAX, |one| one - 1);
    Some((address, bits, number & mask == 0))
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
pub(super) fn ipv4_prefix<T>(op: &'static str, value: &str) -> Result<(), Failure<T>> {
    match prefix(value) {
        Some((IpAddr::V4(_), _, true)) => Ok(()),
        Some((_, _, false)) => Err(input(op, "requires CIDR to have no host bits set")),
        _ => Err(input(op, "requires CIDR to be an IPv4 CIDR prefix")),
    }
}

crate::error::debug_record!(NetworkWriteError; DefaultResource, Busy, UnexpectedStatus);
