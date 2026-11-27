use std::{collections::BTreeMap, time::Duration};

use bytes::Bytes;
use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::value::RawValue;

use crate::model::wire::{WireField, go_json};
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, RouteAuth},
    options::{Options, PathIds},
    redirect::{Classification, Retry},
    response, retry,
};
use crate::wire::strict_json::{Shape, field_matches};
use crate::{ApiError, ContextEnd, Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use super::deletes::{context_error, local, safe_text};
use super::wire::{
    CompleteNotFound, DecodeError, Envelope, Read, invalid_input, screen_input, validate_ids,
};
use super::{Client, WriteError, ZonedLoadBalancer, guard};

pub(super) const INTERVAL: Duration = Duration::from_secs(5);
pub(super) const BOUND: Duration = Duration::from_secs(600);
const LOAD_BALANCER: Read = Read {
    id: "loadbalancer.GetLoadBalancer",
    route: "v2/{}/loadBalancers/{}",
};

pub(super) struct BusyRefusal;
pub(super) struct CompleteRefusal;

pub(super) type WriteResult<T> = Result<T, Failure<T>>;

type Validate = fn(&Read, &str, &Evidence) -> Result<(), Box<Error>>;

pub(super) struct ChildTarget<'a> {
    pub(super) kind: &'static str,
    pub(super) id: &'a str,
    pub(super) read: &'a Read,
    pub(super) ids: &'a [(&'static str, &'a str)],
    pub(super) shape: &'static Shape<'static>,
    pub(super) guard_field: Option<&'static str>,
    pub(super) validate: Validate,
}

pub(super) struct ChildWriter<'a> {
    pub(super) client: &'a Client,
    pub(super) operation: &'static str,
    pub(super) lb_id: &'a str,
    pub(super) evidence: Evidence,
    pub(super) report_redirect: bool,
}

pub(super) struct Ready {
    pub(super) lb_type: String,
    pub(super) child: Option<String>,
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl<'a> ChildWriter<'a> {
    pub(super) fn new(
        client: &'a Client,
        ctx: &RequestContext,
        operation: &'static str,
        lb_id: &'a str,
        ids: &[(&str, &str)],
    ) -> Result<Self, Box<Error>> {
        validate_ids(operation, ids, PathIds::Strict)?;
        let mut jitter = || client.session.retry_jitter();
        let runtime = client.session.runtime(ctx, &mut jitter)?;
        let mut evidence = Evidence::new();
        if let Some(credentials) = runtime.configured_credentials {
            evidence.absorb(credentials);
        }
        screen_input(&evidence, ids.iter().map(|(_, value)| *value))?;
        Ok(Self {
            client,
            operation,
            lb_id,
            evidence,
            report_redirect: false,
        })
    }

    pub(super) async fn read_data(
        &mut self,
        ctx: &RequestContext,
        read: &Read,
        ids: &[(&str, &str)],
    ) -> Result<String, Box<Error>> {
        let text = self
            .client
            .read_text(ctx, read, ids, &[], &mut self.evidence, true)
            .await?;
        let wrapper = Envelope::parse(&text, read, &self.evidence)?;
        Ok(wrapper
            .canonical_raw(read, &self.evidence, "data")?
            .to_owned())
    }

    pub(super) async fn ready(
        &mut self,
        ctx: &RequestContext,
        child: Option<&ChildTarget<'_>>,
    ) -> Result<Ready, Box<Error>> {
        self.ready_with(ctx, child, |_, _, _| Ok(())).await
    }

    pub(super) async fn ready_with(
        &mut self,
        ctx: &RequestContext,
        child: Option<&ChildTarget<'_>>,
        validate_raw: impl Fn(&Read, &str, &Evidence) -> Result<(), Box<Error>>,
    ) -> Result<Ready, Box<Error>> {
        let clock = self.client.session.clock();
        let wait = ctx.child_with_deadline(clock.monotonic_now() + BOUND);
        loop {
            self.ready_time(ctx, &wait, child)?;
            let lb = self.parent(&wait).await;
            self.ready_time(ctx, &wait, child)?;
            let (status, lb_type) = lb?;
            if !lb_busy(&status) {
                let Some(child) = child else {
                    return Ok(Ready {
                        lb_type,
                        child: None,
                    });
                };
                let row = self.read_data(&wait, child.read, child.ids).await;
                self.ready_time(ctx, &wait, Some(child))?;
                let row = row?;
                if let Some(field) = child.guard_field {
                    validate_guard_string(child.read, &row, field, &self.evidence)?;
                }
                validate_raw(child.read, &row, &self.evidence)?;
                let row = guard::canonical_json(child.read, &row, child.shape, &self.evidence)?;
                (child.validate)(child.read, &row, &self.evidence)?;
                if !child_busy(&progress(child.read, &row, child.id, &self.evidence)?) {
                    return Ok(Ready {
                        lb_type,
                        child: Some(row),
                    });
                }
            }
            wait.child_with_deadline(clock.monotonic_now() + INTERVAL)
                .wait(clock)
                .await;
        }
    }

    pub(super) fn ready_time(
        &self,
        ctx: &RequestContext,
        wait: &RequestContext,
        child: Option<&ChildTarget<'_>>,
    ) -> Result<(), Box<Error>> {
        let clock = self.client.session.clock();
        if let Some(end) = ctx.check(clock) {
            return Err(Box::new(context_error(end)));
        }
        if wait.check(clock).is_none() {
            return Ok(());
        }
        let resource = child.map_or_else(String::new, |child| {
            format!(" or {} {}", child.kind, safe_text(child.id, &self.evidence))
        });
        let parent = safe_text(self.lb_id, &self.evidence);
        let message = format!(
            "loadbalancer: resource busy: {}: load balancer {parent}{resource} is not ready within 10m0s; nothing sent",
            self.operation
        );
        Err(Box::new(
            local(ErrorClass::Api, &message, &self.evidence)
                .with_outer_detail(WriteError::ResourceBusy),
        ))
    }

    pub(super) async fn parent(
        &mut self,
        ctx: &RequestContext,
    ) -> Result<(String, String), Box<Error>> {
        let data = self
            .read_data(ctx, &LOAD_BALANCER, &[("LoadBalancerID", self.lb_id)])
            .await?;
        let ZonedLoadBalancer(parent) =
            guard::parse(&LOAD_BALANCER, &data, &guard::LOAD_BALANCER, &self.evidence)?;
        validate_guard_string(&LOAD_BALANCER, &data, "type", &self.evidence)?;
        screen_input(&self.evidence, [parent.r#type.as_str()])?;
        Ok((
            progress(&LOAD_BALANCER, &data, self.lb_id, &self.evidence)?,
            parent.r#type,
        ))
    }

    pub(super) async fn send<O>(
        &mut self,
        ctx: &RequestContext,
        method: Method,
        target: &ChildTarget<'_>,
        body: Bytes,
        busy_child: Option<&ChildTarget<'_>>,
    ) -> Result<(StatusCode, Bytes), Failure<O>> {
        let retry = if method == Method::POST {
            Retry::NonIdempotent
        } else {
            Retry::Repeatable
        };
        let first = self
            .send_once(ctx, &method, target, body.clone(), retry)
            .await;
        match first {
            Err(failure) if failure.error().detail::<BusyRefusal>().is_some() => {
                self.ready(ctx, busy_child)
                    .await
                    .map_err(|error| Failure::new(*error, None, failure.write_dispatch()))?;
                self.send_once(ctx, &method, target, body, retry)
                    .await
                    .map_err(|failure: Failure<O>| {
                        Failure::new(failure.into_parts().0, None, WriteDispatch::MayHaveBeenSent)
                    })
            }
            first => first,
        }
    }

    pub(super) async fn send_once<O>(
        &mut self,
        ctx: &RequestContext,
        method: &Method,
        target: &ChildTarget<'_>,
        body: Bytes,
        retry: Retry,
    ) -> Result<(StatusCode, Bytes), Failure<O>> {
        let session = &self.client.session;
        let mut jitter = || session.retry_jitter();
        let runtime = session.runtime(ctx, &mut jitter).map_err(not_sent)?;
        screen_body(&body, &self.evidence).map_err(not_sent)?;
        let project = session
            .require_project_id_with_evidence(ctx, &mut self.evidence)
            .await
            .map_err(|error| {
                Failure::new(
                    self.evidence.screen_error(*error),
                    None,
                    WriteDispatch::NotSent,
                )
            })?;
        let path: Vec<_> = std::iter::once(project.as_str())
            .chain(target.ids.iter().map(|(_, id)| *id))
            .collect();
        screen_input(&self.evidence, path.iter().copied()).map_err(not_sent)?;
        screen_body(&body, &self.evidence).map_err(not_sent)?;
        let base = session.endpoints().vlb.as_ref().ok_or_else(|| {
            Failure::new(
                Error::new(ErrorClass::InvalidConfig),
                None,
                WriteDispatch::NotSent,
            )
        })?;
        let op = Operation {
            id: self.operation,
            template: target.read.route,
            method: method.clone(),
            class: Classification::Write,
            retry,
            success: &[StatusCode::OK, StatusCode::ACCEPTED],
            cap: 8 << 20,
            allow_recovery: true,
            allow_empty: true,
            no_output: false,
            keep_status: false,
            overflow_overrides_status: true,
            sensitive: false,
            reject_reflected_credentials: false,
            no_redirect: true,
            retry_config: retry::Config::default(),
        };
        let reply = operation::execute_raw(
            runtime,
            base,
            &op,
            &Options {
                report_redirect: self.report_redirect,
                ..Options::DEFAULT
            },
            &path,
            &[],
            RouteAuth::Bearer(session.bearer_source()),
            body,
            &mut self.evidence,
        )
        .await
        .map_err(|failure| {
            let (error, _, dispatch) = failure.into_parts();
            Failure::new(self.evidence.screen_error(error), None, dispatch)
        })?;
        let status = reply.status;
        let body = Bytes::from(reply.reply.body);
        let refusal = status.is_client_error();
        let busy = refusal
            && response::inspect_error_message(&body, |message| {
                !response::reflects_credential_forms(message, self.evidence.credentials())
                    && busy_message(message)
            })
            .unwrap_or(false);
        let body = response::classify(
            op.id,
            status,
            body,
            op.success,
            (true, false, op.retry),
            self.evidence.credentials(),
        )
        .map_err(|error| {
            let mut error = self.evidence.screen_error(*error);
            if refusal {
                error = error.with_outer_detail(CompleteRefusal);
            }
            if busy {
                error = error.with_outer_detail(BusyRefusal);
            }
            Failure::new(error, None, WriteDispatch::MayHaveBeenSent)
        })?;
        Ok((status, body))
    }

    pub(super) fn create_failure<O>(&self, failure: Failure<O>, list: &str) -> Failure<O> {
        if failure.write_dispatch() == WriteDispatch::NotSent
            || failure.error().detail::<CompleteRefusal>().is_some()
            || failure.error().detail::<CompleteNotFound>().is_some()
        {
            return failure;
        }
        let (error, partial, dispatch) = failure.into_parts();
        let cause = public_reason(&error);
        let message = format!(
            "{}: create may have already reached the server; check with {list} and match the name exactly: {cause}",
            self.operation
        );
        Failure::new(
            local(error.class(), &message, &self.evidence).with_cause(error),
            partial,
            dispatch,
        )
    }

    pub(super) fn acknowledged_id<O>(
        &self,
        status: StatusCode,
        body: &Bytes,
        list: &str,
        no_wait: bool,
    ) -> Result<String, Failure<O>> {
        let text = go_json(body);
        let read = Read {
            id: self.operation,
            route: "",
        };
        let parsed_id = if body.is_empty() {
            Ok(String::new())
        } else {
            crate::wire::strict_json::validate(text.as_bytes(), &[], text.len())
                .map_err(|_| read.decode_error(&self.evidence, DecodeError::InvalidShape))
                .and_then(|()| Envelope::parse(&text, &read, &self.evidence))
                .and_then(|fields| {
                    canonical_field::<String>(&fields, &read, &self.evidence, "uuid")
                })
        };
        let id = parsed_id.map_err(|error| {
            self.create_failure(
                Failure::new(*error, None, WriteDispatch::MayHaveBeenSent),
                list,
            )
        })?;
        let valid = if no_wait {
            PathIds::WithUnderscore
        } else {
            PathIds::Strict
        };
        let id = (valid.allows(&id) && !self.evidence.id_reflects(&id)).then_some(id);
        id.ok_or_else(|| {
            let message = format!("create response had no id; the resource may exist, check with {list} and match the name exactly");
            let error = self.evidence.screen_error(Error::from_api(ApiError::without_fallback(
                self.operation, Some(status.as_u16()), crate::error::SafeText::new("", &[]),
                crate::error::SafeText::new(&message, &[]), false,
            )));
            Failure::new(error, None, WriteDispatch::MayHaveBeenSent)
        })
    }

    pub(super) fn unconfirmed(&self, target: &ChildTarget<'_>, cause: Error) -> Box<Error> {
        let id = safe_text(target.id, &self.evidence);
        let reason = public_reason(&cause);
        let message = format!(
            "vngcloud: write outcome not confirmed: {}: {} {id}: {reason}",
            self.operation, target.kind
        );
        Box::new(local(ErrorClass::NotSettled, &message, &self.evidence).with_cause(cause))
    }

    pub(super) fn failed(&self, kind: &str, id: &str) -> Box<Error> {
        let id = safe_text(id, &self.evidence);
        let message = format!(
            "vngcloud: write failed: {}: {kind} {id} is ERROR",
            self.operation
        );
        Box::new(local(ErrorClass::Failed, &message, &self.evidence))
    }

    pub(super) fn confirm_time(
        &self,
        ctx: &RequestContext,
        wait: &RequestContext,
        target: &ChildTarget<'_>,
    ) -> Result<(), Box<Error>> {
        let Some(end) = wait.check(self.client.session.clock()) else {
            return Ok(());
        };
        let reason = match end {
            ContextEnd::Cancelled => "confirmation cancelled by caller",
            ContextEnd::DeadlineExceeded if ctx.deadline() == wait.deadline() => {
                "confirmation stopped at the caller's deadline"
            }
            ContextEnd::DeadlineExceeded => "did not settle within 10m0s",
        };
        let id = safe_text(target.id, &self.evidence);
        let message = format!(
            "vngcloud: write outcome not confirmed: {}: {} {id} {reason}; the write was accepted and must not be repeated",
            self.operation, target.kind
        );
        let error = local(ErrorClass::NotSettled, &message, &self.evidence);
        let error = if let Some(end) = ctx.check(self.client.session.clock()) {
            error.with_cause(context_error(end))
        } else {
            error
        };
        Err(Box::new(error))
    }
}

pub(super) fn progress(
    read: &Read,
    data: &str,
    id: &str,
    evidence: &Evidence,
) -> Result<String, Box<Error>> {
    let fields = Envelope::parse(data, read, evidence)?;
    let uuid: String = canonical_field(&fields, read, evidence, "uuid")?;
    let progress: String = canonical_field(&fields, read, evidence, "progressStatus")?;
    if uuid != id || progress.is_empty() {
        return Err(read.decode_error(evidence, DecodeError::InvalidShape));
    }
    screen_input(evidence, [uuid.as_str(), progress.as_str()])?;
    Ok(progress)
}

pub(super) fn child_busy(progress: &str) -> bool {
    matches!(progress, "CREATING" | "UPDATING" | "DELETING")
}

pub(super) fn lb_busy(progress: &str) -> bool {
    progress == "CREATING-BILLING" || child_busy(progress)
}

fn validate_guard_string(
    read: &Read,
    body: &str,
    field: &str,
    evidence: &Evidence,
) -> Result<(), Box<Error>> {
    let invalid = || read.decode_error(evidence, DecodeError::InvalidShape);
    let fields: BTreeMap<String, &RawValue> = serde_json::from_str(body).map_err(|_| invalid())?;
    if fields.into_iter().any(|(name, value)| {
        field_matches(field, &name) && (name != field || !value.get().starts_with('"'))
    }) {
        return Err(invalid());
    }
    Ok(())
}

fn busy_message(message: &str) -> bool {
    [
        b"is not ready".as_slice(),
        b"is updating",
        b"is creating",
        b"is deleting",
    ]
    .iter()
    .any(|phrase| {
        message
            .as_bytes()
            .windows(phrase.len())
            .any(|part| part.eq_ignore_ascii_case(phrase))
    })
}

pub(super) fn canonical_field<T: WireField>(
    fields: &Envelope<'_>,
    read: &Read,
    evidence: &Evidence,
    name: &str,
) -> Result<T, Box<Error>> {
    read.parse(fields.canonical_raw(read, evidence, name)?, evidence)
}

pub(super) fn encode_body(body: &impl Serialize, evidence: &Evidence) -> Result<Bytes, Box<Error>> {
    let body = Bytes::from(
        serde_json::to_vec(body).map_err(|_| invalid_input("cannot encode request body"))?,
    );
    screen_body(&body, evidence)?;
    Ok(body)
}

fn screen_body(body: &Bytes, evidence: &Evidence) -> Result<(), Box<Error>> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| invalid_input("cannot encode request body"))?;
    if evidence.output_reflects(&value) || evidence.text_reflects(&String::from_utf8_lossy(body)) {
        return Err(invalid_input("request contains credential material"));
    }
    Ok(())
}

#[expect(
    clippy::boxed_local,
    reason = "Private request errors are boxed until building a public Failure"
)]
pub(super) fn not_sent<O>(error: Box<Error>) -> Failure<O> {
    Failure::new(*error, None, WriteDispatch::NotSent)
}

fn public_reason(error: &Error) -> String {
    let Some(api) = error.api_error() else {
        return "request failed".into();
    };
    if error.detail::<DecodeError>().is_some() && !api.message().is_empty() {
        return api.message().into();
    }
    api_error_text(api)
}

fn api_error_text(api: &ApiError) -> String {
    use std::fmt::Write as _;

    let mut message = String::new();
    if !api.operation().is_empty() {
        let _ = write!(message, "{}: ", api.operation());
    }
    message.push_str(if api.message().is_empty() {
        "request failed"
    } else {
        api.message()
    });
    if let Some(status) = api.status() {
        let _ = write!(message, " (status {status})");
    }
    message
}
