use crate::wire::product::guard::guarded_poll;
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
    retry,
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
    Created(&'a crate::wire::product::guard::AcknowledgedId),
    Deleted,
    Status(&'static str),
    Rebooted(tokio::time::Instant),
    Resized(&'a str),
}

impl Settlement<'_> {
    pub(super) fn matches(&self, server: &Server, now: tokio::time::Instant) -> bool {
        let has_status = |status| server.status.eq_ignore_ascii_case(status);
        match self {
            Self::Created(_) => has_status("ACTIVE"),
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
    ) -> Result<crate::wire::product::sender::Response, Failure<O>> {
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
            no_output: !matches!(
                mutation.id,
                "compute.CreateServer"
                    | "compute.RenameServer"
                    | "compute.CreateServerGroup"
                    | "compute.CreateSSHKey"
                    | "compute.ImportSSHKey"
            ),
            keep_status: false,
            raw_body: false,
            overflow_overrides_status: mutation.overflow_overrides_status,
            sensitive: !matches!(policy, ReplyPolicy::Ordinary),
            reflection: crate::wire::product::operation::Reflection::Redact,
            no_redirect: matches!(
                mutation.id,
                "compute.CreateServer"
                    | "compute.StartServer"
                    | "compute.StopServer"
                    | "compute.RebootServer"
                    | "compute.ResizeServer"
                    | "compute.RenameServer"
                    | "compute.ImportSSHKey"
                    | "compute.CreateSSHKey"
            ),
            report_redirect: matches!(
                mutation.id,
                "compute.CreateServer"
                    | "compute.StartServer"
                    | "compute.StopServer"
                    | "compute.RebootServer"
                    | "compute.ResizeServer"
                    | "compute.RenameServer"
                    | "compute.ImportSSHKey"
                    | "compute.CreateSSHKey"
            ),
            retry_config: retry::Config::default(),
        };
        let reply = operation::execute_raw(
            runtime,
            base,
            &operation,
            &Options {
                receipt_policy: if matches!(policy, ReplyPolicy::PrivateKey) {
                    crate::wire::product::guard::ReceiptPolicy::Transport
                } else {
                    crate::wire::product::guard::ReceiptPolicy::All
                },
                ..Options::DEFAULT
            },
            &path,
            &[],
            RouteAuth::Bearer(self.session.bearer_source()),
            body,
            evidence,
        )
        .await
        .map_err(|failure| {
            let (error, _, dispatch) = failure.into_parts();
            let error = match status(&error) {
                Some(status)
                    if error.is_body_too_large()
                        && matches!(
                            mutation.id,
                            "compute.CreateServer"
                                | "compute.RenameServer"
                                | "compute.ImportSSHKey"
                                | "compute.CreateSSHKey"
                        ) =>
                {
                    api(
                        mutation.id,
                        Some(status),
                        "InvalidResponse",
                        "invalid server response; body withheld",
                        evidence,
                    )
                    .with_cause(error)
                }
                _ => error,
            };
            Failure::new(evidence.screen_error(error), None, dispatch)
        })?;
        if operation.success.contains(&reply.status) {
            if matches!(
                mutation.id,
                "compute.CreateServer" | "compute.RenameServer" | "compute.CreateServerGroup"
            ) && !reply.empty_body()
                && reply.malformed_json()
            {
                let error = operation::go_decode_failure(
                    mutation.id,
                    mutation.id != "compute.CreateServerGroup",
                );
                return Err(Failure::new(error, None, WriteDispatch::MayHaveBeenSent));
            }
            return Ok(reply);
        }
        let withhold = matches!(policy, ReplyPolicy::PrivateKey)
            || (matches!(policy, ReplyPolicy::SshPublic)
                && !operation.success.contains(&reply.status)
                && reply
                    .clean_receipt()
                    .is_none_or(|receipt| super::ssh_keys::withhold_key_error(receipt.body())));
        reply
            .classify(
                operation.id,
                operation.success,
                (true, withhold, operation.retry),
                false,
            )
            .map_err(|error| {
                let error = error.with_clean_absence(reply.clean_absence());
                let error = if matches!(policy, ReplyPolicy::SshPublic) && !withhold {
                    super::ssh_keys::key_already_gone(&reply, ids, error)
                } else {
                    error
                };
                Failure::new(
                    evidence.screen_error(error),
                    None,
                    WriteDispatch::MayHaveBeenSent,
                )
            })?;
        Ok(reply)
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
            Settlement::Created(_) | Settlement::Resized(_) => 15,
            Settlement::Deleted => 10,
            Settlement::Status(_) | Settlement::Rebooted(_) => 5,
        };
        let wait =
            ctx.child_with_deadline(clock.monotonic_now() + Duration::from_secs(minutes * 60));
        guarded_poll!(evidence, {
            check(&wait, clock)?;
            if let Settlement::Created(receipt) = mode {
                receipt.recheck(evidence)?;
            }
            let result = self.server_guard::<()>(&wait, id, evidence).await;
            check(&wait, clock)?;
            if let Settlement::Created(receipt) = mode {
                receipt.recheck(evidence)?;
            }
            match result {
                Err(error)
                    if crate::confirms_absence(
                        error.error(),
                        super::inventory::GET_SERVER.id,
                        "GET",
                        super::inventory::GET_SERVER.template,
                    ) =>
                {
                    match mode {
                        Settlement::Deleted => return Ok(()),
                        Settlement::Created(_) => {}
                        _ => return Err(Box::new(error.into_parts().0)),
                    }
                }
                Err(error) => return Err(Box::new(error.into_parts().0)),
                Ok(server) => {
                    let server = server.into_checked(evidence)?;
                    let invalid = |evidence| {
                        Box::new(api(
                            "compute.GetServer",
                            Some(200),
                            "InvalidResponse",
                            "invalid server response; body withheld",
                            evidence,
                        ))
                    };
                    if evidence.output_reflects(&json!([server.uuid, server.status]))
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
        });
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
        if evidence.output_reflects(value) || !evidence.can_retain_partial() {
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
    let reason = error.api_error().map(ApiError::go_text).unwrap_or_else(|| {
        match error.class() {
            ErrorClass::Failed => "server reached ERROR",
            ErrorClass::Cancelled => "context canceled",
            ErrorClass::DeadlineExceeded => "context deadline exceeded",
            _ => "",
        }
        .to_owned()
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
        error.with_outer_detail(advice)
    }
}

pub(super) fn rejected(error: &Error) -> bool {
    !error.is_body_too_large() && status(error).is_some_and(|status| (400..500).contains(&status))
}

pub(super) fn status(error: &Error) -> Option<u16> {
    error.api_error().and_then(ApiError::observed_status)
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
