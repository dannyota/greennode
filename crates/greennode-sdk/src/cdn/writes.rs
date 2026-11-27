mod settle;
mod update;

use std::collections::BTreeMap;

use bytes::Bytes;
use http::Method;
use serde::Serialize;
use serde_json::{Value, json, value::RawValue};

use super::{
    Client, ErrorDetail, WebAccelerator, models::redact_text, reads::WebDetail, status_name,
    wire::Call,
};
use crate::wire::product::{evidence::Evidence, options::PathIds, redirect::Retry};
use crate::wire::strict_json::field_matches;
use crate::{ApiError, Error, ErrorClass, Failure, RequestContext, Secret, WriteDispatch};

pub use update::{
    RuleActionInput, UpdateWebAcceleratorInput, UpdateWebAcceleratorOutput, UpstreamInput,
};

const DELETE: Call = Call::write(
    "cdn.DeleteWebAccelerator",
    "v1/cdn/delete/{}",
    &Method::DELETE,
);
const ENABLE: Call = Call::write(
    "cdn.EnableWebAccelerator",
    "v1/cdn/status/change/{}",
    &Method::PUT,
);
const DISABLE: Call = Call::write(
    "cdn.DisableWebAccelerator",
    "v1/cdn/status/change/{}",
    &Method::PUT,
);
const PURGE: Call = Call {
    retry: Retry::NonIdempotent,
    ..Call::write("cdn.PurgePaths", "v1/cdn/flush-cache", &Method::POST)
};

macro_rules! toggle_types {
    ($input:ident, $output:ident) => {
        /// Input for changing a Web Accelerator's enabled state.
        #[derive(Clone)]
        pub struct $input {
            /// CDN identifier.
            pub cdn_id: String,
            /// Skips settlement after a read confirms the toggle began.
            pub no_wait: bool,
        }

        crate::error::debug_record!($input { cdn_id, no_wait });

        impl $input {
            /// Creates the input with settlement enabled.
            pub fn new(cdn_id: impl Into<String>) -> Self {
                Self {
                    cdn_id: cdn_id.into(),
                    no_wait: false,
                }
            }
        }

        /// Result of changing a Web Accelerator's enabled state.
        #[derive(Clone, Serialize)]
        #[serde(rename_all = "PascalCase")]
        pub struct $output {
            /// Last confirmed Web Accelerator detail.
            pub web_accelerator: WebAccelerator,
            /// Whether a toggle was sent and confirmed by a read.
            pub changed: bool,
        }

        crate::wire::product::output::public_fields!($output {
            web_accelerator,
            changed
        });
    };
}

toggle_types!(EnableWebAcceleratorInput, EnableWebAcceleratorOutput);
toggle_types!(DisableWebAcceleratorInput, DisableWebAcceleratorOutput);

/// Input for deleting a Web Accelerator.
#[derive(Clone)]
pub struct DeleteWebAcceleratorInput {
    /// CDN identifier.
    pub cdn_id: String,
}

crate::error::debug_record!(DeleteWebAcceleratorInput { cdn_id });

impl DeleteWebAcceleratorInput {
    /// Creates the input with the CDN identifier.
    pub fn new(cdn_id: impl Into<String>) -> Self {
        Self {
            cdn_id: cdn_id.into(),
        }
    }
}

/// An accepted Web Accelerator deletion.
#[derive(Clone, Serialize)]
pub struct DeleteWebAcceleratorOutput {}

crate::wire::product::output::public_fields!(DeleteWebAcceleratorOutput {});

/// Input for purging exact cached paths.
#[derive(Clone)]
pub struct PurgePathsInput {
    /// Generated CDN domain.
    pub cdn_domain: String,
    /// Nonempty paths without wildcard asterisks.
    pub paths: Vec<String>,
}

crate::error::debug_record!(PurgePathsInput { cdn_domain, paths });

impl PurgePathsInput {
    /// Creates the input with a generated CDN domain and its paths.
    pub fn new(cdn_domain: impl Into<String>, paths: Vec<String>) -> Self {
        Self {
            cdn_domain: cdn_domain.into(),
            paths,
        }
    }
}

/// An accepted purge of the requested paths.
#[derive(Clone, Serialize)]
pub struct PurgePathsOutput {}

crate::wire::product::output::public_fields!(PurgePathsOutput {});

#[derive(Clone, Copy, PartialEq, Eq)]
enum WriteKind {
    Update,
    Delete,
    Enable,
    Disable,
}

impl Client {
    /// Deletes an active or disabled Web Accelerator after verifying its identity and status.
    pub async fn delete_web_accelerator(
        &self,
        ctx: &RequestContext,
        input: DeleteWebAcceleratorInput,
    ) -> Result<DeleteWebAcceleratorOutput, Failure<DeleteWebAcceleratorOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = self
                .write_evidence(ctx, None, Some(&input.cdn_id))
                .map_err(super::wire::convert)?;
            self.preflight(
                ctx,
                &DELETE,
                &input.cdn_id,
                WriteKind::Delete,
                &mut evidence,
            )
            .await
            .map_err(super::wire::convert)?;
            if let Err(failure) = self
                .call_with_evidence(ctx, &DELETE, &[&input.cdn_id], Bytes::new(), &mut evidence)
                .await
            {
                let failure = self
                    .explain_401(ctx, &DELETE, &input.cdn_id, failure, &mut evidence)
                    .await;
                return Err(super::wire::convert(possibly_applied(failure, "delete")));
            }
            Ok(DeleteWebAcceleratorOutput {})
        })
    }

    /// Enables a disabled Web Accelerator and confirms that its status changed.
    pub async fn enable_web_accelerator(
        &self,
        ctx: &RequestContext,
        input: EnableWebAcceleratorInput,
    ) -> Result<EnableWebAcceleratorOutput, Failure<EnableWebAcceleratorOutput>> {
        crate::wire::product::exit::finish!(async {
            self.toggle(
                ctx,
                &ENABLE,
                &input.cdn_id,
                WriteKind::Enable,
                input.no_wait,
                |web_accelerator, changed| EnableWebAcceleratorOutput {
                    web_accelerator,
                    changed,
                },
            )
            .await
        })
    }

    /// Disables an active Web Accelerator and confirms that its status changed.
    pub async fn disable_web_accelerator(
        &self,
        ctx: &RequestContext,
        input: DisableWebAcceleratorInput,
    ) -> Result<DisableWebAcceleratorOutput, Failure<DisableWebAcceleratorOutput>> {
        crate::wire::product::exit::finish!(async {
            self.toggle(
                ctx,
                &DISABLE,
                &input.cdn_id,
                WriteKind::Disable,
                input.no_wait,
                |web_accelerator, changed| DisableWebAcceleratorOutput {
                    web_accelerator,
                    changed,
                },
            )
            .await
        })
    }

    /// Purges exact paths. Retries only a failed dial or HTTP 429.
    /// A rejected purge may report a cooldown.
    pub async fn purge_paths(
        &self,
        ctx: &RequestContext,
        input: PurgePathsInput,
    ) -> Result<PurgePathsOutput, Failure<PurgePathsOutput>> {
        crate::wire::product::exit::finish!(async {
            if input.cdn_domain.is_empty()
                || input.paths.is_empty()
                || input
                    .paths
                    .iter()
                    .any(|path| path.is_empty() || path.contains('*'))
            {
                return Err(invalid(
                    "cdn.PurgePaths requires a CDNDomain and nonempty Paths without *",
                ));
            }
            let supplied = json!([input.cdn_domain, input.paths]);
            let mut evidence = self
                .write_evidence(ctx, Some(&supplied), None)
                .map_err(super::wire::convert)?;
            let body = json!({"cdnDomain":input.cdn_domain,"type":"URI","patterns":input.paths});
            self.call_with_evidence(ctx, &PURGE, &[], encode(&body)?, &mut evidence)
                .await
                .map_err(super::wire::convert)?;
            Ok(PurgePathsOutput {})
        })
    }

    fn write_evidence(
        &self,
        ctx: &RequestContext,
        body: Option<&Value>,
        cdn_id: Option<&str>,
    ) -> Result<Evidence, Failure<()>> {
        if cdn_id.is_some_and(|id| !PathIds::Strict.allows(id)) {
            return Err(invalid(
                "CDNID must contain only ASCII letters, digits, or hyphens",
            ));
        }
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|err| before_send(*err))?;
        let mut evidence = Evidence::new();
        if let Some(configured) = runtime.configured_credentials {
            evidence.absorb(configured);
        }
        if cdn_id.is_some_and(|id| evidence.output_reflects(&Value::String(id.into())))
            || body.is_some_and(|body| evidence.output_reflects(body))
        {
            return Err(invalid("the write input contains a configured credential"));
        }
        Ok(evidence)
    }

    async fn preflight(
        &self,
        ctx: &RequestContext,
        call: &Call,
        cdn_id: &str,
        kind: WriteKind,
        evidence: &mut Evidence,
    ) -> Result<(WebDetail, bool), Failure<()>> {
        let detail = self
            .read_guard_detail(ctx, call.id, cdn_id, evidence)
            .await
            .map_err(|failure| before_send(failure.into_parts().0))?;
        let detail = detail
            .into_checked(evidence)
            .map_err(|error| before_send(*error))?;
        let send = match detail.status {
            0 if kind == WriteKind::Update => {
                return Err(invalid(
                    "cdn.UpdateWebAccelerator: the CDN is DISABLED; enable it first",
                ));
            }
            0 => kind != WriteKind::Disable,
            1 => kind != WriteKind::Enable,
            3..=5 => {
                return Err(before_send(outcome(
                    ErrorDetail::ResourceBusy,
                    format!(
                        "cdn: CDN is changing status: {}: the CDN is {}; nothing was sent, run the call again after it settles",
                        call.id,
                        safe_status(detail.status, evidence)
                    ),
                )));
            }
            status => {
                let mut status = status.to_string();
                redact_text(&mut status, evidence);
                return Err(before_send(outcome(
                    ErrorDetail::UnexpectedStatus,
                    format!(
                        "cdn: unexpected CDN status: {}: the CDN has status {status}; nothing was sent",
                        call.id
                    ),
                )));
            }
        };
        Ok((detail, send))
    }

    async fn explain_401(
        &self,
        ctx: &RequestContext,
        call: &Call,
        cdn_id: &str,
        failure: Failure<()>,
        evidence: &mut Evidence,
    ) -> Failure<()> {
        if failure.error().api_error().and_then(ApiError::status) == Some(401)
            && let Err(read) = self.read_guard_detail(ctx, call.id, cdn_id, evidence).await
            && read.error().has_class(ErrorClass::NotFound)
        {
            return Failure::new(read.into_parts().0, None, failure.write_dispatch());
        }
        failure
    }
}

fn before_send<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotSent)
}

fn invalid<T>(message: &'static str) -> Failure<T> {
    before_send(Error::new(ErrorClass::InvalidInput).with_detail(message.to_owned()))
}

fn outcome(detail: ErrorDetail, message: String) -> Error {
    Error::new(ErrorClass::Api)
        .with_detail(detail)
        .with_outer_detail(message)
}

fn encode<T>(body: &impl Serialize) -> Result<Bytes, Failure<T>> {
    serde_json::to_vec(body)
        .map(Bytes::from)
        .map_err(|_| invalid("the CDN write body could not be encoded"))
}

fn possibly_applied(failure: Failure<()>, action: &str) -> Failure<()> {
    if rejected(&failure) {
        return failure;
    }
    let (error, _, dispatch) = failure.into_parts();
    let message = error
        .api_error()
        .map(ApiError::message)
        .filter(|text| !text.is_empty())
        .unwrap_or("request failed");
    let advice = format!(
        "{message}; the {action} may have been applied: read the CDN before running it again"
    );
    Failure::new(error.with_outer_detail(advice), None, dispatch)
}

fn rejected(failure: &Failure<()>) -> bool {
    failure.write_dispatch() != WriteDispatch::MayHaveBeenSent
        || failure
            .error()
            .api_error()
            .is_some_and(|api| match api.status() {
                Some(400..=499) => true,
                Some(200..=299) => api.code() != "EmptyResponse",
                _ => false,
            })
}

fn redact(mut detail: WebDetail, evidence: &mut Evidence) -> WebAccelerator {
    record_user_uuids(detail.raw.get(), evidence);
    detail.model.redact(evidence);
    detail.model
}

fn record_user_uuids(raw: &str, evidence: &mut Evidence) {
    if let Ok(fields) = serde_json::from_str::<BTreeMap<String, &RawValue>>(raw) {
        for (key, item) in fields {
            if field_matches(&key, "userUuid")
                && let Ok(uuid) = serde_json::from_str::<String>(item.get())
            {
                evidence.add_redaction(&Secret::new(uuid));
            }
        }
    }
}

fn safe_status(status: i64, evidence: &Evidence) -> String {
    let mut name = status_name(status);
    redact_text(&mut name, evidence);
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::product::retry::{Config, Outcome, SendFailure, decide};
    use std::time::SystemTime;

    #[test]
    fn purge_retries_a_proven_failed_dial_but_never_an_ambiguous_send() {
        let config = Config::default();
        for (failure, retry) in [
            (SendFailure::FailedDial, true),
            (SendFailure::PossiblySent, false),
        ] {
            assert_eq!(
                decide(
                    PURGE.retry,
                    0,
                    &config,
                    Outcome::Send(failure),
                    SystemTime::UNIX_EPOCH,
                    0
                )
                .is_some(),
                retry
            );
        }
    }
}
