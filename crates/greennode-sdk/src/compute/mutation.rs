use std::time::Duration;

use bytes::Bytes;
use http::{Method, StatusCode};
use serde_json::{Value, json};

use crate::model::wire::WireField;
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, RouteAuth},
    options::Options,
    redirect::{Classification, Retry},
    response, retry,
};
use crate::{ApiError, ContextEnd, Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use super::wire::api;
use super::{Client, Server};

pub(super) struct Mutation {
    pub(super) id: &'static str,
    pub(super) template: &'static str,
    pub(super) method: Method,
    pub(super) retry: Retry,
}

pub(super) enum Settlement {
    Created,
    Deleted,
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    pub(super) async fn send_mutation<O>(
        &self,
        ctx: &RequestContext,
        mutation: &Mutation,
        ids: &[&str],
        body: &Value,
        sensitive: bool,
        evidence: &mut Evidence,
    ) -> Result<Bytes, Failure<O>> {
        let project = self
            .session
            .require_project_id_with_evidence(ctx, evidence)
            .await
            .map_err(|error| not_sent(*error))?;
        let path: Vec<_> = std::iter::once(project.as_str())
            .chain(ids.iter().copied())
            .collect();
        let body = serde_json::to_vec(body)
            .map(Bytes::from)
            .map_err(|_| not_sent(Error::new(ErrorClass::InvalidInput)))?;
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|error| not_sent(*error))?;
        let base = self
            .session
            .endpoints()
            .vserver
            .as_ref()
            .ok_or_else(|| not_sent(Error::new(ErrorClass::InvalidConfig)))?;
        let repeatable = matches!(mutation.retry, Retry::Repeatable);
        let operation = Operation {
            id: mutation.id,
            template: mutation.template,
            method: mutation.method.clone(),
            class: Classification::Write,
            retry: mutation.retry,
            success: &[StatusCode::ACCEPTED],
            cap: 8 << 20,
            allow_recovery: repeatable,
            allow_empty: true,
            no_output: false,
            keep_status: true,
            overflow_overrides_status: !repeatable,
            sensitive,
            reject_reflected_credentials: false,
            no_redirect: !repeatable,
            retry_config: retry::Config::default(),
        };
        let reply = operation::execute_raw(
            runtime,
            base,
            &operation,
            &Options::DEFAULT,
            &path,
            &[],
            RouteAuth::Bearer(self.session.bearer_source()),
            body,
            evidence,
        )
        .await
        .map_err(|failure| {
            let (error, _, dispatch) = failure.into_parts();
            Failure::new(
                error.screen_api(&|text| evidence.text_reflects(text)),
                None,
                dispatch,
            )
        })?;
        response::classify(
            operation.id,
            reply.status,
            Bytes::copy_from_slice(&reply.body),
            operation.success,
            (true, false, operation.retry),
            evidence.credentials(),
        )
        .map_err(|error| {
            Failure::new(
                error.screen_api(&|text| evidence.text_reflects(text)),
                None,
                WriteDispatch::MayHaveBeenSent,
            )
        })
    }

    pub(super) async fn settle_server(
        &self,
        ctx: &RequestContext,
        id: &str,
        mode: Settlement,
        latest: &mut Server,
        evidence: &mut Evidence,
    ) -> Result<(), Box<Error>> {
        let clock = self.session.clock();
        let minutes = match mode {
            Settlement::Created => 15,
            Settlement::Deleted => 10,
        };
        let wait =
            ctx.child_with_deadline(clock.monotonic_now() + Duration::from_secs(minutes * 60));
        loop {
            check(&wait, clock)?;
            let result = self.server_detail::<()>(&wait, id, evidence).await;
            check(&wait, clock)?;
            match result {
                Err(error)
                    if error.error().detail::<StatusCode>().is_none()
                        && error.error().api_error().is_some_and(|api| {
                            api.operation() == "compute.GetServer" && api.status() == Some(404)
                        }) =>
                {
                    if matches!(mode, Settlement::Deleted) {
                        return Ok(());
                    }
                }
                Err(error) => return Err(Box::new(error.into_parts().0)),
                Ok(server) => {
                    let invalid = |evidence| {
                        Box::new(api(
                            "compute.GetServer",
                            Some(200),
                            "InvalidResponse",
                            "invalid server response; body withheld",
                            evidence,
                        ))
                    };
                    if evidence.reflected_in(&json!([server.uuid, server.status])) {
                        return Err(invalid(evidence));
                    }
                    *latest = server;
                    if latest.status.eq_ignore_ascii_case("ERROR") {
                        return Err(Box::new(Error::new(ErrorClass::Failed)));
                    }
                    let status = match mode {
                        Settlement::Created => "ACTIVE",
                        Settlement::Deleted => "DELETED",
                    };
                    if latest.status.eq_ignore_ascii_case(status) {
                        return Ok(());
                    }
                }
            }
            let tick = wait.child_with_deadline(clock.monotonic_now() + Duration::from_secs(5));
            if tick.wait(clock).await == ContextEnd::Cancelled {
                return Err(Box::new(ContextEnd::Cancelled.into()));
            }
        }
    }

    pub(super) fn guard_write(
        &self,
        ctx: &RequestContext,
        value: &Value,
        evidence: &mut Evidence,
    ) -> Result<(), Box<Error>> {
        let mut jitter = || self.session.retry_jitter();
        let runtime = self.session.runtime(ctx, &mut jitter)?;
        if let Some(credentials) = runtime.configured_credentials {
            evidence.absorb(credentials);
        }
        if evidence.reflected_in(value) || !evidence.can_retain_partial() {
            return Err(Box::new(Error::new(ErrorClass::InvalidInput)));
        }
        Ok(())
    }
}

fn check(ctx: &RequestContext, clock: &dyn crate::Clock) -> Result<(), Box<Error>> {
    ctx.check(clock)
        .map_or(Ok(()), |end| Err(Box::new(end.into())))
}

pub(super) fn not_sent<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotSent)
}

pub(super) fn settlement_error(
    operation: &'static str,
    error: Error,
    message: &str,
    evidence: &Evidence,
) -> Error {
    let class = if error.has_class(ErrorClass::Failed) {
        ErrorClass::Failed
    } else {
        ErrorClass::NotSettled
    };
    let code = if class == ErrorClass::Failed {
        "WriteFailed"
    } else {
        "NotSettled"
    };
    Error::new(class).with_cause(error).with_cause(api(
        "",
        None,
        code,
        &format!("{operation}: {message}"),
        evidence,
    ))
}

pub(super) fn status(error: &Error) -> Option<u16> {
    error
        .api_error()
        .and_then(ApiError::status)
        .or_else(|| error.detail::<StatusCode>().map(|value| value.as_u16()))
}

pub(super) fn safe_server(mut server: Server, evidence: &Evidence) -> Option<Server> {
    (evidence.can_retain_partial() && server.redact(evidence)).then_some(server)
}
