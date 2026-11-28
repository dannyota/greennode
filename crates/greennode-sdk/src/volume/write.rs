use crate::wire::product::guard::{AcknowledgedId, guarded_poll};
use std::time::Duration;

use http::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, Output},
    options::Options,
    redirect::{Classification, Retry},
    retry, sender,
};
use crate::wire::strict_json::{Shape, field_matches};
use crate::{
    ContextEnd, Error, ErrorClass, Failure, RecoveryAction, RecoveryAdvice, RequestContext,
    WriteDispatch,
};

use super::wire::{CAP, Object, Redact, api, invalid};
use super::{Client, Volume};

#[derive(Clone)]
pub(super) struct Mutation {
    pub(super) id: &'static str,
    pub(super) route: &'static str,
    pub(super) method: Method,
    pub(super) once: bool,
}

#[derive(Default, Deserialize)]
pub(super) struct Acknowledgement {
    #[serde(skip)]
    pub(super) receipt: Option<AcknowledgedId>,
}

impl Output for Acknowledgement {
    const SCHEMA: Shape<'static> = Shape::Any;

    fn decode_response(
        operation: &Operation,
        reply: &sender::Response,
    ) -> Result<Self, Box<Error>> {
        reply.classify(
            operation.id,
            operation.success,
            (true, operation.sensitive, operation.retry),
            false,
        )?;
        if operation.method != Method::POST {
            reply.check_discarded_acknowledgement(operation.id)?;
            return Ok(Self::default());
        }
        if reply.empty_body() {
            return Ok(Self::default());
        }
        let malformed = || {
            Box::new(Error::from_api(
                crate::ApiError::without_fallback(
                    operation.id,
                    None,
                    crate::error::SafeText::new("", &[]),
                    crate::error::SafeText::new("", &[]),
                    false,
                )
                .with_request_cause(crate::error::SafeText::new(
                    "response failed to decode",
                    &[],
                )),
            ))
        };
        let noncanonical = || {
            Box::new(api(
                operation.id,
                Some(reply.status.as_u16()),
                "InvalidResponse",
                "create acknowledgement had noncanonical fields",
            ))
        };
        if reply.malformed_json() {
            return Err(malformed());
        }
        let receipt = reply.clean_receipt().ok_or_else(|| {
            Box::new(api(
                operation.id,
                Some(reply.status.as_u16()),
                "InvalidResponse",
                "create acknowledgement withheld",
            ))
        })?;
        let raw: Box<serde_json::value::RawValue> =
            serde_json::from_slice(receipt.body()).map_err(|_| malformed())?;
        crate::wire::strict_json::validate(raw.get().as_bytes(), &[], CAP)
            .map_err(|_| malformed())?;
        let envelope = Object::parse(&raw, crate::wire::strict_json::FieldNames::Compatible)
            .map_err(|_| malformed())?;
        let Some(data) = envelope.exact("data").map_err(|_| noncanonical())? else {
            return Ok(Self::default());
        };
        let data = Object::parse(data, crate::wire::strict_json::FieldNames::Compatible)
            .map_err(|_| malformed())?;
        let uuid = data
            .exact("uuid")
            .map_err(|_| noncanonical())?
            .map(|field| serde_json::from_str(field.get()))
            .transpose()
            .map_err(|_| malformed())?;
        let Some(uuid) = uuid else {
            return Ok(Self::default());
        };
        Ok(Self {
            receipt: Some(receipt.bind_id(uuid)),
        })
    }
}

pub(super) const CREATE_UNKNOWN: &str = "create response had no id";
const WAIT_BOUND: Duration = Duration::from_secs(300);
const POLL_INTERVAL: Duration = Duration::from_secs(2);

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    pub(super) async fn send_write<O>(
        &self,
        ctx: &RequestContext,
        mutation: &Mutation,
        path: &[&str],
        body: Option<&Value>,
        evidence: &mut Evidence,
    ) -> Result<Acknowledgement, Failure<O>> {
        self.guard_inputs(ctx, evidence, path, body)
            .map_err(not_sent)?;
        let base = self
            .session
            .endpoints()
            .vserver
            .as_ref()
            .ok_or_else(|| not_sent(Box::new(Error::new(ErrorClass::InvalidConfig))))?;
        let mut jitter = || self.session.retry_jitter();
        let runtime = self.session.runtime(ctx, &mut jitter).map_err(not_sent)?;
        let body = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|_| not_sent(invalid("vngcloud: invalid input: invalid request body")))?
            .map_or_else(bytes::Bytes::new, bytes::Bytes::from);
        let operation = Operation {
            id: mutation.id,
            template: mutation.route,
            method: mutation.method.clone(),
            class: Classification::Write,
            retry: if mutation.once {
                Retry::Once
            } else {
                Retry::Repeatable
            },
            success: &[StatusCode::ACCEPTED],
            cap: CAP,
            allow_recovery: !mutation.once,
            allow_empty: true,
            no_output: mutation.method != Method::POST,
            keep_status: false,
            raw_body: false,
            overflow_overrides_status: false,
            sensitive: false,
            reflection: crate::wire::product::operation::Reflection::Redact,
            no_redirect: false,
            report_redirect: mutation.once,
            retry_config: retry::Config::default(),
        };
        operation::execute_with_evidence::<Acknowledgement>(
            runtime,
            base,
            &operation,
            &Options::DEFAULT,
            path,
            &[],
            self.session.bearer_source(),
            body,
            evidence,
        )
        .await
        .map(|(_, acknowledgement)| acknowledgement)
        .map_err(|failure| {
            let (error, _, dispatch) = failure.into_parts();
            let error = error.screen_api(&|text| evidence.text_reflects(text));
            Failure::new(error, None, dispatch)
        })
    }

    pub(super) fn guard_inputs(
        &self,
        ctx: &RequestContext,
        evidence: &mut Evidence,
        path: &[&str],
        body: Option<&Value>,
    ) -> Result<(), Box<Error>> {
        let values = serde_json::json!([path, body]);
        self.guard_body(ctx, evidence, &values)
    }

    pub(super) async fn settle(
        &self,
        ctx: &RequestContext,
        op: &str,
        id: &str,
        settlement: Settlement<'_>,
        latest: &mut Volume,
        evidence: &mut Evidence,
    ) -> Result<(), Box<Error>> {
        let clock = self.session.clock();
        let deadline = clock.monotonic_now() + WAIT_BOUND;
        let wait = ctx.child_with_deadline(deadline);
        guarded_poll!(evidence, {
            if let Some(end) = ctx.check(clock) {
                return Err(Box::new(end.into()));
            }
            if clock.monotonic_now() >= deadline {
                return Err(settlement.timeout(op, id, None, evidence));
            }
            settlement.check_receipt(evidence)?;
            let read = self.volume(&wait, id, evidence).await;
            #[cfg(any(test, feature = "test-support"))]
            clock.volume_confirmation_read_completed();
            let step = settlement
                .check_receipt(evidence)
                .and_then(|()| settlement.observe(read, latest, evidence));
            let wait_end = wait.check(clock);
            if let Some(end) = ctx.check(clock) {
                return Err(step.err().unwrap_or_else(|| Box::new(end.into())));
            }
            if let Some(end) = wait_end {
                return Err(settlement.timeout(op, id, Some(end), evidence));
            }
            if step? {
                return Ok(());
            }
            let tick = wait.child_with_deadline(clock.monotonic_now() + POLL_INTERVAL);
            tick.wait(clock).await;
            let wait_end = wait.check(clock);
            if let Some(end) = ctx.check(clock) {
                return Err(Box::new(end.into()));
            }
            if let Some(end) = wait_end {
                return Err(settlement.timeout(op, id, Some(end), evidence));
            }
        });
    }
}

pub(super) enum Settlement<'a> {
    Attached(&'a str),
    Detached,
    Deleted,
    Created(&'a AcknowledgedId),
    Resized(u64),
}

impl Settlement<'_> {
    fn check_receipt(&self, evidence: &Evidence) -> Result<(), Box<Error>> {
        match self {
            Self::Created(receipt) => receipt.recheck(evidence).map(drop),
            _ => Ok(()),
        }
    }

    fn observe(
        &self,
        read: Result<Volume, Box<Error>>,
        latest: &mut Volume,
        evidence: &Evidence,
    ) -> Result<bool, Box<Error>> {
        match read {
            Err(error)
                if crate::confirms_absence(
                    &error,
                    "volume.GetVolume",
                    "GET",
                    super::inventory::VOLUME.route,
                ) =>
            {
                match self {
                    Self::Created(_) => Ok(false),
                    Self::Deleted => Ok(true),
                    _ => Err(error),
                }
            }
            Err(error) => Err(error),
            Ok(volume) => {
                screen_state(evidence, &volume)?;
                *latest = volume;
                if latest.status.eq_ignore_ascii_case("ERROR") {
                    Err(Box::new(Error::new(ErrorClass::Failed)))
                } else {
                    Ok(self.matches(latest))
                }
            }
        }
    }

    fn timeout(
        &self,
        op: &str,
        id: &str,
        wait_end: Option<ContextEnd>,
        evidence: &Evidence,
    ) -> Box<Error> {
        let (reason, fallback) = match self {
            Self::Attached(server) => (
                format!(
                    "did not confirm attach to server {server} within 5m0s; run this operation again to check"
                ),
                "Volume attachment confirmation timed out. Run the operation again to check.",
            ),
            Self::Detached => (
                "did not confirm detach within 5m0s; run this operation again to check".into(),
                "Volume detachment confirmation timed out. Run the operation again to check.",
            ),
            Self::Deleted => (
                "did not reach 404 or DELETED within 5m0s; delete was sent and a rerun is safe"
                    .into(),
                "Volume deletion confirmation timed out. Rerunning the deletion is safe.",
            ),
            Self::Created(_) => (
                "did not reach AVAILABLE within 5m0s".into(),
                "Volume creation confirmation timed out.",
            ),
            Self::Resized(size) => (
                format!(
                    "did not confirm resize to {size} GB within 5m0s; call GetVolume to check its status"
                ),
                "Volume resize confirmation timed out. Call GetVolume to check its status.",
            ),
        };
        let mut error = Error::new(ErrorClass::NotSettled);
        let reflects = |text: &str| evidence.text_reflects(text);
        let detailed = format!("vngcloud: write outcome not confirmed: {op}: volume {id} {reason}");
        let concise = format!("Volume {reason}");
        let candidates = [detailed.as_str(), &concise, fallback];
        let advice = if let Some(end) = wait_end {
            error = error.with_cause(end.into());
            let cause = context_message(end);
            RecoveryAdvice::screened_with_cause(&candidates, cause, reflects)
                .or_else(|| RecoveryAdvice::screened(&candidates, reflects))
                .or_else(|| RecoveryAdvice::screened(&[cause], reflects))
        } else {
            RecoveryAdvice::screened(&candidates, reflects)
        };
        if let Some(advice) = advice {
            error = error.with_detail(advice);
        }
        if matches!(self, Self::Created(_)) {
            error = error.with_recovery_action(RecoveryAction::DoNotRepeatVolumeCreate, reflects);
        }
        Box::new(error)
    }

    fn matches(&self, volume: &Volume) -> bool {
        match self {
            Self::Attached(server) => in_use(volume) && attached(volume, server),
            Self::Detached | Self::Created(_) => available(volume),
            Self::Deleted => volume.status.eq_ignore_ascii_case("DELETED"),
            Self::Resized(size) => volume.size == *size && (available(volume) || in_use(volume)),
        }
    }
}

pub(super) fn available(volume: &Volume) -> bool {
    volume.status.eq_ignore_ascii_case("AVAILABLE")
}

pub(super) fn in_use(volume: &Volume) -> bool {
    field_matches(&volume.status, "IN-USE")
}

pub(super) fn attached(volume: &Volume, server: &str) -> bool {
    volume.server_id == server
        || volume
            .server_id_list
            .iter()
            .flatten()
            .any(|id| id == server)
}

pub(super) fn screen_state(evidence: &Evidence, volume: &Volume) -> Result<(), Box<Error>> {
    use crate::wire::product::output::{FieldScan, PublicOutput};
    if volume.public_reflects(&FieldScan::new(evidence), false) {
        return Err(Box::new(api(
            "volume.GetVolume",
            Some(200),
            "InvalidResponse",
            "volume response withheld",
        )));
    }
    Ok(())
}

fn context_message(end: ContextEnd) -> &'static str {
    match end {
        ContextEnd::Cancelled => "context canceled",
        ContextEnd::DeadlineExceeded => "context deadline exceeded",
    }
}

#[expect(
    clippy::boxed_local,
    reason = "Private pre-send errors are boxed until building the public Failure"
)]
pub(super) fn not_sent<T>(error: Box<Error>) -> Failure<T> {
    Failure::new(*error, None, WriteDispatch::NotSent)
}

pub(super) fn client_error(error: &Error) -> bool {
    error
        .api_error()
        .is_some_and(crate::ApiError::is_client_rejection)
}

pub(super) fn named(class: ErrorClass, message: &str) -> Error {
    Error::new(class).with_cause(api("", None, "", message))
}

pub(super) fn settlement_error(
    error: Error,
    op: &str,
    id: &str,
    pre_confirmation: Option<&str>,
    recovery: Option<RecoveryAction>,
    evidence: &Evidence,
) -> Error {
    if error.class() == ErrorClass::NotSettled {
        return error;
    }
    let failed = error.class() == ErrorClass::Failed;
    let class = if failed {
        ErrorClass::Failed
    } else {
        ErrorClass::NotSettled
    };
    let reflects = |text: &str| evidence.text_reflects(text);
    let cause = error.with_service_display().screen_api(&reflects);
    let advice = if failed {
        let detailed = format!("vngcloud: write failed: {op}: volume {id} is ERROR");
        let concise = format!("vngcloud: write failed: {op}: volume is ERROR");
        RecoveryAdvice::screened(&[&detailed, &concise, "Volume write failed."], reflects)
    } else {
        let context = [
            (ErrorClass::Cancelled, ContextEnd::Cancelled),
            (ErrorClass::DeadlineExceeded, ContextEnd::DeadlineExceeded),
        ]
        .into_iter()
        .find_map(|(class, end)| cause.has_identity(class).then_some(context_message(end)));
        let display = cause.api_error().map(crate::ApiError::go_text);
        let reason = display.as_deref().or(context);
        let fallback = reason.is_none().then(|| {
            if cause.class() == ErrorClass::Auth
                && cause.login_error().is_none()
                && cause.service_account_login_error().is_none()
            {
                "vngcloud: authentication failed".into()
            } else {
                cause.to_string()
            }
        });
        let reason = reason.or(fallback.as_deref()).unwrap_or_default();
        let detail = match pre_confirmation {
            Some(advice) => format!("{advice}: {reason}"),
            None => reason.to_owned(),
        };
        let detailed =
            format!("vngcloud: write outcome not confirmed: {op}: volume {id}: {detail}");
        RecoveryAdvice::screened(
            &[&detailed, &detail, reason, "Check the volume state."],
            reflects,
        )
    };
    let error = Error::new(class)
        .with_cause(cause)
        .with_detail(advice.unwrap_or_else(|| RecoveryAdvice::new("")));
    match recovery {
        Some(action) => error.with_recovery_action(action, reflects),
        None => error,
    }
}

pub(super) fn safe<T: Serialize + Redact>(mut output: T, evidence: &Evidence) -> Option<T> {
    output.redact(evidence);
    let screened = evidence.can_retain_partial()
        && serde_json::to_value(&output).is_ok_and(|value| !evidence.output_reflects(&value));
    screened.then_some(output)
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
pub(super) fn finish<T: Serialize + Redact>(
    output: T,
    result: Result<(), Box<Error>>,
    evidence: &Evidence,
    mutation: &Mutation,
    id: &str,
    recovery: Option<RecoveryAction>,
    dispatch: WriteDispatch,
) -> Result<T, Failure<T>> {
    let output = safe(output, evidence);
    match (result, output) {
        (Ok(()), Some(output)) => Ok(output),
        (Ok(()), None) => {
            let reflects = |text: &str| evidence.text_reflects(text);
            let error = api(
                mutation.id,
                None,
                "InvalidResponse",
                "volume response withheld",
            )
            .screen_api(&reflects);
            Err(Failure::new(error, None, dispatch))
        }
        (Err(error), partial) => Err(Failure::new(
            settlement_error(*error, mutation.id, id, None, recovery, evidence),
            partial,
            dispatch,
        )),
    }
}

#[cfg(test)]
mod screening_tests {
    use super::*;
    use crate::Secret;

    #[test]
    fn settlement_rechecks_identifiers_after_a_later_bearer() {
        let mut evidence = Evidence::new();
        let volume = Volume {
            uuid: "volume-1".into(),
            server_id_list: Some(vec!["pre6A6B6Cpost".into()]),
            ..Default::default()
        };
        assert!(screen_state(&evidence, &volume).is_ok());
        evidence.add_bearer(&Secret::new("jkl"));
        assert!(screen_state(&evidence, &volume).is_err());
    }
}
