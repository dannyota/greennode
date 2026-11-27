use std::time::Duration;

use http::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};

use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, Output},
    options::Options,
    redirect::{Classification, Retry},
    response, retry, sender,
};
use crate::wire::strict_json::Shape;
use crate::{ContextEnd, Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use super::wire::{CAP, Redact, api, invalid};
use super::{Client, Volume};

#[derive(Clone)]
pub(super) struct Mutation {
    pub(super) id: &'static str,
    pub(super) route: &'static str,
    pub(super) method: Method,
    pub(super) once: bool,
}

#[derive(Default, Deserialize)]
#[serde(transparent)]
pub(super) struct Acknowledgement(pub(super) Option<Box<RawValue>>);

impl Output for Acknowledgement {
    const SCHEMA: Shape<'static> = Shape::Any;

    fn decode_response(
        operation: &Operation,
        reply: &sender::Response,
    ) -> Result<Self, Box<Error>> {
        let body = response::classify(
            operation.id,
            reply.status,
            bytes::Bytes::copy_from_slice(&reply.body),
            operation.success,
            (true, operation.sensitive, operation.retry),
            &reply.credentials,
        )?;
        if operation.method != Method::POST || body.is_empty() {
            return Ok(Self::default());
        }
        let raw = serde_json::from_slice(&body).map_err(|_| {
            Box::new(api(
                operation.id,
                Some(reply.status.as_u16()),
                "",
                CREATE_UNKNOWN,
            ))
        })?;
        Ok(Self(Some(raw)))
    }
}

pub(super) const CREATE_UNKNOWN: &str = "create response had no id; the volume may exist, list volumes and match the name exactly before creating it again";

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
            keep_status: true,
            overflow_overrides_status: false,
            sensitive: false,
            reflection: crate::wire::product::operation::Reflection::Redact,
            no_redirect: true,
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
        id: &str,
        settlement: Settlement<'_>,
        latest: &mut Volume,
        evidence: &mut Evidence,
    ) -> Result<(), Box<Error>> {
        let clock = self.session.clock();
        let wait = ctx.child_with_deadline(clock.monotonic_now() + Duration::from_secs(300));
        loop {
            check(&wait, clock)?;
            let next = self.volume(&wait, id, evidence).await;
            check(&wait, clock)?;
            match next {
                Err(error)
                    if error.api_error().and_then(crate::ApiError::status) == Some(404)
                        && matches!(settlement, Settlement::Deleted) =>
                {
                    return Ok(());
                }
                Err(error)
                    if error.api_error().and_then(crate::ApiError::status) == Some(404)
                        && matches!(settlement, Settlement::Created) => {}
                Err(error) => return Err(error),
                Ok(volume) => {
                    screen_state(evidence, &volume)?;
                    *latest = volume;
                    if latest.status.eq_ignore_ascii_case("ERROR") {
                        return Err(Box::new(Error::new(ErrorClass::Failed)));
                    }
                    if settlement.matches(latest) {
                        return Ok(());
                    }
                }
            }
            let tick = wait.child_with_deadline(clock.monotonic_now() + Duration::from_secs(2));
            if tick.wait(clock).await == ContextEnd::Cancelled {
                return Err(Box::new(ContextEnd::Cancelled.into()));
            }
        }
    }
}

pub(super) enum Settlement<'a> {
    Attached(&'a str),
    Detached,
    Deleted,
    Created,
    Resized(u64),
}

impl Settlement<'_> {
    fn matches(&self, volume: &Volume) -> bool {
        match self {
            Self::Attached(server) => in_use(volume) && attached(volume, server),
            Self::Detached | Self::Created => available(volume),
            Self::Deleted => volume.status.eq_ignore_ascii_case("DELETED"),
            Self::Resized(size) => volume.size == *size && (available(volume) || in_use(volume)),
        }
    }
}

pub(super) fn available(volume: &Volume) -> bool {
    volume.status.eq_ignore_ascii_case("AVAILABLE")
}

pub(super) fn in_use(volume: &Volume) -> bool {
    volume.status.eq_ignore_ascii_case("IN-USE")
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
    if evidence.output_reflects(&serde_json::json!({
        "uuid": volume.uuid,
        "status": volume.status,
        "size": volume.size,
        "serverId": volume.server_id,
        "serverIds": volume.server_id_list,
        "bootable": volume.bootable,
        "volumeTypeId": volume.volume_type_id,
    })) {
        return Err(Box::new(api(
            "volume.GetVolume",
            Some(200),
            "InvalidResponse",
            "volume response withheld",
        )));
    }
    Ok(())
}

fn check(ctx: &RequestContext, clock: &dyn crate::Clock) -> Result<(), Box<Error>> {
    ctx.check(clock)
        .map_or(Ok(()), |end| Err(Box::new(end.into())))
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
        .and_then(crate::ApiError::status)
        .is_some_and(|status| (400..500).contains(&status))
}

pub(super) fn named(class: ErrorClass, message: &str) -> Error {
    Error::new(class).with_cause(api("", None, "", message))
}

pub(super) fn settlement_error(error: Error, op: &str, detail: &str) -> Error {
    let (class, advice) = if error.class() == ErrorClass::Failed {
        (
            ErrorClass::Failed,
            format!("vngcloud: write failed: {op}: volume is ERROR"),
        )
    } else {
        (
            ErrorClass::NotSettled,
            format!("vngcloud: write outcome not confirmed: {op}: {detail}"),
        )
    };
    Error::new(class)
        .with_detail(crate::RecoveryAdvice::new(&advice))
        .with_cause(error)
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
    detail: &str,
    dispatch: WriteDispatch,
) -> Result<T, Failure<T>> {
    let output = safe(output, evidence);
    match (result, output) {
        (Ok(()), Some(output)) => Ok(output),
        (Ok(()), None) => Err(Failure::new(
            api(
                mutation.id,
                None,
                "InvalidResponse",
                "volume response withheld",
            ),
            None,
            dispatch,
        )),
        (Err(error), partial) => Err(Failure::new(
            settlement_error(*error, mutation.id, detail),
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
