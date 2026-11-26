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
use crate::{
    ApiError, ContextEnd, Error, ErrorClass, Failure, RecoveryAdvice, RequestContext, WriteDispatch,
};

use super::wire::api;
use super::{Client, Server};

pub(super) struct Mutation {
    pub(super) id: &'static str,
    pub(super) template: &'static str,
    pub(super) method: Method,
    pub(super) retry: Retry,
    pub(super) success: &'static [StatusCode],
    pub(super) overflow_overrides_status: bool,
}

#[derive(Clone, Copy)]
pub(super) enum ReplyPolicy {
    Ordinary,
    UserData,
    PrivateKey,
    SshPublic,
}

impl From<bool> for ReplyPolicy {
    fn from(sensitive: bool) -> Self {
        if sensitive {
            Self::UserData
        } else {
            Self::Ordinary
        }
    }
}

pub(super) enum Settlement<'a> {
    Created,
    Deleted,
    Status(&'static str),
    Rebooted(tokio::time::Instant),
    Resized(&'a str),
}

impl Settlement<'_> {
    pub(super) fn matches(&self, server: &Server, now: tokio::time::Instant) -> bool {
        let has_status = |status| server.status.eq_ignore_ascii_case(status);
        match self {
            Self::Created => has_status("ACTIVE"),
            Self::Deleted => has_status("DELETED"),
            Self::Status(status) => has_status(status),
            Self::Rebooted(earliest) => has_status("ACTIVE") && now >= *earliest,
            Self::Resized(flavor) => {
                server.flavor.flavor_id == *flavor
                    && (has_status("ACTIVE") || has_status("STOPPED"))
            }
        }
    }
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
        body: Option<&Value>,
        policy: impl Into<ReplyPolicy>,
        evidence: &mut Evidence,
    ) -> Result<Bytes, Failure<O>> {
        let policy = policy.into();
        let project = self
            .session
            .require_project_id_with_evidence(ctx, evidence)
            .await
            .map_err(|error| not_sent(*error))?;
        let path: Vec<_> = std::iter::once(project.as_str())
            .chain(ids.iter().copied())
            .collect();
        let body = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|_| not_sent(Error::new(ErrorClass::InvalidInput)))?
            .map_or_else(Bytes::new, Bytes::from);
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
        let once = matches!(mutation.retry, Retry::Once);
        let operation = Operation {
            id: mutation.id,
            template: mutation.template,
            method: mutation.method.clone(),
            class: Classification::Write,
            retry: mutation.retry,
            success: mutation.success,
            cap: 8 << 20,
            allow_recovery: !once,
            allow_empty: true,
            no_output: false,
            keep_status: true,
            overflow_overrides_status: mutation.overflow_overrides_status,
            sensitive: !matches!(policy, ReplyPolicy::Ordinary),
            reject_reflected_credentials: false,
            no_redirect: true,
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
        let withhold = matches!(policy, ReplyPolicy::PrivateKey)
            || (matches!(policy, ReplyPolicy::SshPublic)
                && !operation.success.contains(&reply.status)
                && super::ssh_keys::withhold_key_error(&reply.body));
        response::classify(
            operation.id,
            reply.status,
            Bytes::copy_from_slice(&reply.body),
            operation.success,
            (true, withhold, operation.retry),
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
        mode: Settlement<'_>,
        latest: &mut Server,
        evidence: &mut Evidence,
    ) -> Result<(), Box<Error>> {
        let clock = self.session.clock();
        let minutes = match mode {
            Settlement::Created | Settlement::Resized(_) => 15,
            Settlement::Deleted => 10,
            Settlement::Status(_) | Settlement::Rebooted(_) => 5,
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
                    match mode {
                        Settlement::Deleted => return Ok(()),
                        Settlement::Created => {}
                        _ => return Err(Box::new(error.into_parts().0)),
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
                    if evidence.reflected_in(&json!([server.uuid, server.status]))
                        || (matches!(mode, Settlement::Resized(_))
                            && evidence.text_reflects(&server.flavor.flavor_id))
                    {
                        return Err(invalid(evidence));
                    }
                    *latest = server;
                    if latest.status.eq_ignore_ascii_case("ERROR") {
                        return Err(Box::new(Error::new(ErrorClass::Failed)));
                    }
                    if mode.matches(latest, clock.monotonic_now()) {
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
    let reason = error
        .api_error()
        .map(ApiError::message)
        .filter(|message| !message.is_empty())
        .unwrap_or(match error.class() {
            ErrorClass::Failed => "server reached ERROR",
            ErrorClass::Cancelled => "context canceled",
            ErrorClass::DeadlineExceeded => "context deadline exceeded",
            _ => "",
        });
    let advice = if reason.is_empty() {
        format!("{operation}: {message}")
    } else {
        format!("{operation}: {message}: {reason}")
    };
    let code = if class == ErrorClass::Failed {
        "WriteFailed"
    } else {
        "NotSettled"
    };
    let error = Error::new(class).with_cause(error).with_cause(api(
        "",
        None,
        code,
        &format!("{operation}: {message}"),
        evidence,
    ));
    with_screened_advice(error, &advice, evidence)
}

pub(super) fn with_screened_advice(error: Error, message: &str, evidence: &Evidence) -> Error {
    let advice = RecoveryAdvice::new(message);
    if advice.message().is_empty()
        || evidence.text_reflects(message)
        || evidence.text_reflects(advice.message())
    {
        error
    } else {
        error.with_detail(advice)
    }
}

pub(super) fn rejected(error: &Error) -> bool {
    error.detail::<StatusCode>().is_none()
        && status(error).is_some_and(|status| (400..500).contains(&status))
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

pub(super) fn decode_data<T: WireField>(
    body: &[u8],
    mutation: &Mutation,
    evidence: &Evidence,
) -> Result<T, Box<Error>> {
    use crate::model::wire::{Field, go_json};
    use crate::wire::strict_json;

    #[derive(serde::Deserialize)]
    struct Envelope<'a> {
        #[serde(borrow)]
        data: Option<&'a serde_json::value::RawValue>,
    }

    let text = go_json(body);
    let invalid = || Box::new(invalid_mutation_response(mutation, evidence));
    let raw: &serde_json::value::RawValue = serde_json::from_str(&text).map_err(|_| {
        Box::new(api(
            mutation.id,
            None,
            "",
            "response failed to decode; body withheld",
            evidence,
        ))
    })?;
    strict_json::validate(
        text.as_bytes(),
        &[strict_json::Field {
            name: "data",
            children: &[],
        }],
        8 << 20,
    )
    .map_err(|_| invalid())?;
    if !raw.get().trim_start().starts_with('{') {
        return Err(invalid());
    }
    let envelope: Option<Envelope<'_>> = serde_json::from_str(raw.get()).map_err(|_| invalid())?;
    let Some(data) = envelope.and_then(|envelope| envelope.data) else {
        return Ok(T::default());
    };
    serde_json::from_str::<Field<T>>(data.get())
        .map(|field| field.0)
        .map_err(|_| invalid())
}

pub(super) fn invalid_mutation_response(mutation: &Mutation, evidence: &Evidence) -> Error {
    api(
        mutation.id,
        mutation.success.first().map(StatusCode::as_u16),
        "InvalidResponse",
        "invalid server response; body withheld",
        evidence,
    )
}
