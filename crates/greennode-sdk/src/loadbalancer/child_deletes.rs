use std::{collections::BTreeSet, time::Duration};

use bytes::Bytes;
use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::value::RawValue;

use crate::model::wire::WireField;
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, RouteAuth},
    options::{Options, PathIds},
    redirect::{Classification, Retry},
    response, retry,
};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{ContextEnd, Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use super::deletes::{context_error, local, safe_text};
use super::wire::{
    CompleteNotFound, DecodeError, Envelope, PoolInUseRefusal, Read, pool_in_use_refusal,
    screen_input, validate_ids,
};
use super::{Client, Listener, Policy, Pool, WriteError, ZonedLoadBalancer, guard};

const LOAD_BALANCER: Read = Read {
    id: "loadbalancer.GetLoadBalancer",
    route: "v2/{}/loadBalancers/{}",
};
const LISTENERS: Read = Read {
    id: "loadbalancer.ListListeners",
    route: "v2/{}/loadBalancers/{}/listeners",
};
const INTERVAL: Duration = Duration::from_secs(5);
const BOUND: Duration = Duration::from_secs(10 * 60);

macro_rules! shape {
    ($($key:literal),* ; $($nested:literal => $shape:expr),* $(,)?) => {
        Shape::Object(&[
            $(ShapeField { name: $key, shape: Shape::Any },)*
            $(ShapeField { name: $nested, shape: $shape },)*
        ])
    };
}

const MONITOR: Shape<'_> = shape!("timeout", "createdAt", "updatedAt", "domainName",
    "httpVersion", "healthCheckProtocol", "interval", "healthyThreshold", "unhealthyThreshold",
    "healthCheckMethod", "healthCheckPath", "successCode", "progressStatus", "displayStatus";);
const MEMBER: Shape<'_> = shape!("uuid", "address", "protocolPort", "weight", "monitorPort",
    "subnetId", "name", "poolId", "typeCreate", "backup", "displayStatus", "createdAt",
    "updateAt", "createdBy", "progressStatus";);
const POOL: Shape<'_> = shape!("uuid", "name", "protocol", "description", "loadBalanceMethod",
    "displayStatus", "progressStatus", "stickiness", "tlsEncryption";
    "members" => Shape::Array(&MEMBER), "healthMonitor" => MONITOR);
const HEADER: Shape<'_> = shape!("headerName", "headerValue";);
const LISTENER: Shape<'_> = shape!("uuid", "name", "description", "protocol", "protocolPort",
    "connectionLimit", "defaultPoolId", "defaultPoolName", "timeoutClient", "timeoutMember",
    "timeoutConnection", "allowedCidrs", "certificateAuthorities", "displayStatus", "createdAt",
    "updatedAt", "defaultCertificateAuthority", "clientCertificateAuthentication", "progressStatus";
    "insertHeaders" => Shape::Array(&HEADER));
const RULE: Shape<'_> = shape!("uuid", "compareType", "ruleValue", "ruleType", "provisioningStatus", "operatingStatus";);
const POLICY: Shape<'_> = shape!("uuid", "name", "description", "redirectPoolId", "redirectPoolName",
    "action", "redirectUrl", "redirectHttpCode", "keepQueryString", "position", "displayStatus",
    "createdAt", "updatedAt", "progressStatus"; "l7Rules" => Shape::Array(&RULE));

struct BusyRefusal;

/// Identifies a pool to delete and controls confirmation.
#[derive(Clone, Debug, Default)]
pub struct DeletePoolInput {
    /// Parent load balancer identifier.
    pub load_balancer_id: String,
    /// Target pool identifier.
    pub pool_id: String,
    /// Returns after the provider accepts the delete.
    pub no_wait: bool,
}

/// A pool delete has no output fields.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DeletePoolOutput {}

/// Identifies a listener to delete and controls confirmation.
#[derive(Clone, Debug, Default)]
pub struct DeleteListenerInput {
    /// Parent load balancer identifier.
    pub load_balancer_id: String,
    /// Target listener identifier.
    pub listener_id: String,
    /// Returns after the provider accepts the delete.
    pub no_wait: bool,
}

/// A listener delete has no output fields.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DeleteListenerOutput {}

/// Identifies a policy to delete and controls confirmation.
#[derive(Clone, Debug, Default)]
pub struct DeletePolicyInput {
    /// Parent load balancer identifier.
    pub load_balancer_id: String,
    /// Parent listener identifier.
    pub listener_id: String,
    /// Target policy identifier.
    pub policy_id: String,
    /// Returns after the provider accepts the delete.
    pub no_wait: bool,
}

/// A policy delete has no output fields.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DeletePolicyOutput {}

struct Child<'a> {
    operation: &'static str,
    kind: &'static str,
    read: Read,
    ids: &'a [(&'static str, &'a str)],
    load_balancer: &'a str,
    id: &'a str,
}

type ChildFailure<T> = Result<T, Failure<T>>;

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Deletes an unused pool and confirms that it is absent.
    pub async fn delete_pool(
        &self,
        ctx: &RequestContext,
        input: DeletePoolInput,
    ) -> ChildFailure<DeletePoolOutput> {
        self.delete_child(
            ctx,
            Child {
                operation: "loadbalancer.DeletePool",
                kind: "pool",
                read: Read {
                    id: "loadbalancer.GetPool",
                    route: "v2/{}/loadBalancers/{}/pools/{}",
                },
                ids: &[
                    ("LoadBalancerID", &input.load_balancer_id),
                    ("PoolID", &input.pool_id),
                ],
                load_balancer: &input.load_balancer_id,
                id: &input.pool_id,
            },
            input.no_wait,
        )
        .await
    }

    /// Deletes a listener and confirms that it is absent.
    pub async fn delete_listener(
        &self,
        ctx: &RequestContext,
        input: DeleteListenerInput,
    ) -> ChildFailure<DeleteListenerOutput> {
        self.delete_child(
            ctx,
            Child {
                operation: "loadbalancer.DeleteListener",
                kind: "listener",
                read: Read {
                    id: "loadbalancer.GetListener",
                    route: "v2/{}/loadBalancers/{}/listeners/{}",
                },
                ids: &[
                    ("LoadBalancerID", &input.load_balancer_id),
                    ("ListenerID", &input.listener_id),
                ],
                load_balancer: &input.load_balancer_id,
                id: &input.listener_id,
            },
            input.no_wait,
        )
        .await
    }

    /// Deletes an L7 policy and confirms that it is absent.
    pub async fn delete_policy(
        &self,
        ctx: &RequestContext,
        input: DeletePolicyInput,
    ) -> ChildFailure<DeletePolicyOutput> {
        self.delete_child(
            ctx,
            Child {
                operation: "loadbalancer.DeletePolicy",
                kind: "policy",
                read: Read {
                    id: "loadbalancer.GetPolicy",
                    route: "v2/{}/loadBalancers/{}/listeners/{}/l7policies/{}",
                },
                ids: &[
                    ("LoadBalancerID", &input.load_balancer_id),
                    ("ListenerID", &input.listener_id),
                    ("PolicyID", &input.policy_id),
                ],
                load_balancer: &input.load_balancer_id,
                id: &input.policy_id,
            },
            input.no_wait,
        )
        .await
    }

    async fn delete_child<O: Default>(
        &self,
        ctx: &RequestContext,
        child: Child<'_>,
        no_wait: bool,
    ) -> ChildFailure<O> {
        let mut evidence = Evidence::new();
        validate_ids(child.operation, child.ids, PathIds::Strict)
            .map_err(|error| not_sent(*error))?;
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|error| not_sent(*error))?;
        if let Some(credentials) = runtime.configured_credentials {
            evidence.absorb(credentials);
        }
        screen_input(&evidence, child.ids.iter().map(|(_, id)| *id))
            .map_err(|error| not_sent(*error))?;
        let _lock = self
            .lock_load_balancer(ctx, child.load_balancer)
            .await
            .map_err(not_sent)?;
        if child.kind == "pool" {
            self.check_pool_in_use(ctx, &child, &mut evidence)
                .await
                .map_err(|error| not_sent(*error))?;
        }
        self.wait_child_ready(ctx, &child, &mut evidence)
            .await
            .map_err(|error| not_sent(*error))?;

        let first = self.send_child_delete(ctx, &child, &mut evidence).await;
        match first {
            Err(failure) if failure.error().detail::<BusyRefusal>().is_some() => {
                self.wait_child_ready(ctx, &child, &mut evidence)
                    .await
                    .map_err(|error| {
                        Failure::new(
                            child.in_use_refusal(*error, &evidence),
                            None,
                            failure.write_dispatch(),
                        )
                    })?;
                self.send_child_delete(ctx, &child, &mut evidence)
                    .await
                    .map_err(|failure| {
                        Failure::new(failure.into_parts().0, None, WriteDispatch::MayHaveBeenSent)
                    })
            }
            first => first,
        }
        .map_err(|failure| {
            let (error, _, dispatch) = failure.into_parts();
            Failure::new(error, None, dispatch)
        })?;
        if !no_wait && let Err(error) = self.confirm_child_deleted(ctx, &child, &mut evidence).await
        {
            let partial = evidence.can_retain_partial().then(O::default);
            return Err(Failure::new(
                *error,
                partial,
                WriteDispatch::MayHaveBeenSent,
            ));
        }
        Ok(O::default())
    }

    async fn check_pool_in_use(
        &self,
        ctx: &RequestContext,
        child: &Child<'_>,
        evidence: &mut Evidence,
    ) -> Result<(), Box<Error>> {
        let text = self
            .read_text(
                ctx,
                &LISTENERS,
                &[("LoadBalancerID", child.load_balancer)],
                &[],
                evidence,
                true,
            )
            .await?;
        let wrapper = Envelope::parse(&text, &LISTENERS, evidence)?;
        let raw = wrapper.canonical_raw(&LISTENERS, evidence, "data")?;
        if raw == "null" {
            return Err(Box::new(LISTENERS.api(
                "",
                "list response had no data",
                evidence,
            )));
        }
        let rows: Vec<&RawValue> = serde_json::from_str(raw)
            .map_err(|_| LISTENERS.decode_error(evidence, DecodeError::InvalidShape))?;
        let mut seen = BTreeSet::new();
        let mut used_by = None;
        for row in rows {
            let fields = Envelope::parse(row.get(), &LISTENERS, evidence)?;
            let id: String = canonical_field(&fields, &LISTENERS, evidence, "uuid")?;
            let default: String = canonical_field(&fields, &LISTENERS, evidence, "defaultPoolId")?;
            if (!evidence.text_reflects(&id) && !PathIds::WithUnderscore.allows(&id))
                || (!default.is_empty() && !PathIds::WithUnderscore.allows(&default))
                || !seen.insert(id.clone())
            {
                return Err(LISTENERS.decode_error(evidence, DecodeError::InvalidShape));
            }
            screen_input(evidence, [default.as_str()])?;
            if default == child.id {
                used_by.get_or_insert(id);
            }
        }
        if let Some(id) = used_by {
            let id = safe_text(&id, evidence);
            let message = format!(
                "loadbalancer: resource in use: {}: pool {} is used in listener {id} as its default pool",
                child.operation, child.id
            );
            return Err(Box::new(
                local(ErrorClass::Api, &message, evidence)
                    .with_outer_detail(WriteError::ResourceInUse),
            ));
        }
        Ok(())
    }

    async fn wait_child_ready(
        &self,
        ctx: &RequestContext,
        child: &Child<'_>,
        evidence: &mut Evidence,
    ) -> Result<(), Box<Error>> {
        let clock = self.session.clock();
        let wait = ctx.child_with_deadline(clock.monotonic_now() + BOUND);
        let lb_ids = [("LoadBalancerID", child.load_balancer)];
        loop {
            check_ready_time(ctx, &wait, clock, child, evidence)?;
            let lb = self
                .read_progress(
                    &wait,
                    &LOAD_BALANCER,
                    &lb_ids,
                    child.load_balancer,
                    evidence,
                )
                .await;
            check_ready_time(ctx, &wait, clock, child, evidence)?;
            if !busy(&lb?, true) {
                let status = self
                    .read_progress(&wait, &child.read, child.ids, child.id, evidence)
                    .await;
                check_ready_time(ctx, &wait, clock, child, evidence)?;
                if !busy(&status?, false) {
                    return Ok(());
                }
            }
            wait.child_with_deadline(clock.monotonic_now() + INTERVAL)
                .wait(clock)
                .await;
        }
    }

    async fn confirm_child_deleted(
        &self,
        ctx: &RequestContext,
        child: &Child<'_>,
        evidence: &mut Evidence,
    ) -> Result<(), Box<Error>> {
        let clock = self.session.clock();
        let wait = ctx.child_with_deadline(clock.monotonic_now() + BOUND);
        let lb_ids = [("LoadBalancerID", child.load_balancer)];
        loop {
            check_confirm_time(ctx, &wait, clock, child, evidence)?;
            let status = self
                .read_progress(&wait, &child.read, child.ids, child.id, evidence)
                .await;
            check_confirm_time(ctx, &wait, clock, child, evidence)?;
            match status {
                Err(error) if error.detail::<CompleteNotFound>().is_some() => {
                    let lb = self
                        .read_progress(
                            &wait,
                            &LOAD_BALANCER,
                            &lb_ids,
                            child.load_balancer,
                            evidence,
                        )
                        .await;
                    check_confirm_time(ctx, &wait, clock, child, evidence)?;
                    let lb = lb.map_err(|error| child.unconfirmed(evidence, *error))?;
                    if lb == "ERROR" {
                        return Err(child.failed("load balancer", child.load_balancer, evidence));
                    }
                    if !busy(&lb, true) {
                        return Ok(());
                    }
                }
                Err(error) => return Err(child.unconfirmed(evidence, *error)),
                Ok(status) if status == "ERROR" => {
                    return Err(child.failed(child.kind, child.id, evidence));
                }
                Ok(_) => {}
            }
            wait.child_with_deadline(clock.monotonic_now() + INTERVAL)
                .wait(clock)
                .await;
        }
    }

    async fn read_progress(
        &self,
        ctx: &RequestContext,
        read: &Read,
        ids: &[(&str, &str)],
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<String, Box<Error>> {
        let text = self.read_text(ctx, read, ids, &[], evidence, true).await?;
        let wrapper = Envelope::parse(&text, read, evidence)?;
        let data = wrapper.canonical_raw(read, evidence, "data")?;
        macro_rules! typed_progress {
            ($model:ty, $shape:expr $(, $inner:tt)?) => {{
                let row: $model = guard::parse(read, data, &$shape, evidence)?;
                (row$(.$inner)?.uuid, row$(.$inner)?.progress_status)
            }};
        }
        let (uuid, progress) = match read.id {
            "loadbalancer.GetPool" => typed_progress!(Pool, POOL),
            "loadbalancer.GetListener" => typed_progress!(Listener, LISTENER),
            "loadbalancer.GetPolicy" => typed_progress!(Policy, POLICY),
            _ => typed_progress!(ZonedLoadBalancer, guard::LOAD_BALANCER, 0),
        };
        if uuid != id || progress.is_empty() {
            return Err(read.decode_error(evidence, DecodeError::InvalidShape));
        }
        screen_input(evidence, [uuid.as_str(), progress.as_str()])?;
        Ok(progress)
    }

    async fn send_child_delete(
        &self,
        ctx: &RequestContext,
        child: &Child<'_>,
        evidence: &mut Evidence,
    ) -> ChildFailure<()> {
        let session = &self.session;
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(ctx, &mut jitter)
            .map_err(|error| not_sent(*error))?;
        let project = session
            .require_project_id_with_evidence(ctx, evidence)
            .await
            .map_err(|error| not_sent(error.screen_api(&|value| evidence.text_reflects(value))))?;
        let path: Vec<_> = std::iter::once(project.as_str())
            .chain(child.ids.iter().map(|(_, id)| *id))
            .collect();
        screen_input(evidence, path.iter().copied()).map_err(|error| not_sent(*error))?;
        let base = session
            .endpoints()
            .vlb
            .as_ref()
            .ok_or_else(|| not_sent(Error::new(ErrorClass::InvalidConfig)))?;
        let operation = Operation {
            id: child.operation,
            template: child.read.route,
            method: Method::DELETE,
            class: Classification::Write,
            retry: Retry::Repeatable,
            success: &[StatusCode::OK, StatusCode::ACCEPTED],
            cap: 8 << 20,
            allow_recovery: true,
            allow_empty: true,
            no_output: true,
            keep_status: false,
            overflow_overrides_status: true,
            sensitive: false,
            reject_reflected_credentials: false,
            no_redirect: true,
            retry_config: retry::Config::default(),
        };
        let result = operation::execute_raw(
            runtime,
            base,
            &operation,
            &Options::DEFAULT,
            &path,
            &[],
            RouteAuth::Bearer(session.bearer_source()),
            Bytes::new(),
            evidence,
        )
        .await;
        let screen = |error: Error| error.screen_api(&|value| evidence.text_reflects(value));
        let reply = result.map_err(|failure| {
            let (error, _, dispatch) = failure.into_parts();
            Failure::new(screen(error), None, dispatch)
        })?;
        let status = reply.status;
        let body = Bytes::from(reply.reply.body);
        let busy = status.is_client_error()
            && response::inspect_error_message(&body, |message| {
                message_has_phrase(
                    message,
                    &[
                        b"is not ready",
                        b"is updating",
                        b"is creating",
                        b"is deleting",
                    ],
                )
            })
            .unwrap_or(false);
        let in_use = (child.kind == "pool" && !operation.success.contains(&status))
            .then(|| pool_in_use_refusal(&body, evidence))
            .flatten();
        let classify = if evidence.credentials().values().is_empty() {
            response::classify_full
        } else {
            response::classify
        };
        classify(
            operation.id,
            status,
            body,
            operation.success,
            (true, false, operation.retry),
            evidence.credentials(),
        )
        .map(|_| ())
        .map_err(|error| {
            let mut error = screen(*error);
            if let Some(in_use) = in_use {
                error = child.in_use_refusal(error.with_outer_detail(in_use), evidence);
            }
            if busy {
                error = error.with_outer_detail(BusyRefusal);
            }
            Failure::new(error, None, WriteDispatch::MayHaveBeenSent)
        })
    }
}

impl Child<'_> {
    fn in_use_refusal(&self, error: Error, evidence: &Evidence) -> Error {
        let Some(PoolInUseRefusal(_private_message)) = error
            .detail::<PoolInUseRefusal>()
            .filter(|_| self.kind == "pool")
        else {
            return error;
        };
        let id = safe_text(self.id, evidence);
        let reason = error
            .api_error()
            .map_or_else(String::new, |api| format!(": {}", api_error_text(api)));
        let message = format!(
            "loadbalancer: resource in use: {}: pool {id}{reason}",
            self.operation
        );
        local(ErrorClass::Api, &message, evidence)
            .with_outer_detail(WriteError::ResourceInUse)
            .with_cause(error)
    }

    fn failed(&self, kind: &str, id: &str, evidence: &Evidence) -> Box<Error> {
        let id = safe_text(id, evidence);
        let message = format!(
            "vngcloud: write failed: {}: {kind} {id} is ERROR",
            self.operation
        );
        Box::new(local(ErrorClass::Failed, &message, evidence))
    }

    fn unconfirmed(&self, evidence: &Evidence, cause: Error) -> Box<Error> {
        let id = safe_text(self.id, evidence);
        let reason = match cause.api_error() {
            Some(api) if cause.detail::<DecodeError>().is_some() && !api.message().is_empty() => {
                api.message().to_owned()
            }
            Some(api) => api_error_text(api),
            None => "confirmation read failed".into(),
        };
        let message = format!(
            "vngcloud: write outcome not confirmed: {}: {} {id}: {reason}",
            self.operation, self.kind
        );
        Box::new(local(ErrorClass::NotSettled, &message, evidence).with_cause(cause))
    }
}

fn api_error_text(api: &crate::ApiError) -> String {
    let message = api.message();
    let message = if message.is_empty() {
        "request failed"
    } else {
        message
    };
    match (api.operation(), api.status()) {
        ("", None) => message.into(),
        ("", Some(status)) => format!("{message} (status {status})"),
        (operation, None) => format!("{operation}: {message}"),
        (operation, Some(status)) => format!("{operation}: {message} (status {status})"),
    }
}

fn check_ready_time(
    parent: &RequestContext,
    wait: &RequestContext,
    clock: &dyn crate::Clock,
    child: &Child<'_>,
    evidence: &Evidence,
) -> Result<(), Box<Error>> {
    if let Some(end) = parent.check(clock) {
        return Err(Box::new(context_error(end)));
    }
    if wait.check(clock).is_some() {
        let lb = safe_text(child.load_balancer, evidence);
        let id = safe_text(child.id, evidence);
        let message = format!(
            "loadbalancer: resource busy: {}: load balancer {lb} or {} {id} is not ready within 10m0s; nothing sent",
            child.operation, child.kind
        );
        return Err(Box::new(
            local(ErrorClass::Api, &message, evidence).with_outer_detail(WriteError::ResourceBusy),
        ));
    }
    Ok(())
}

fn check_confirm_time(
    parent: &RequestContext,
    wait: &RequestContext,
    clock: &dyn crate::Clock,
    child: &Child<'_>,
    evidence: &Evidence,
) -> Result<(), Box<Error>> {
    let Some(end) = wait.check(clock) else {
        return Ok(());
    };
    let reason = match end {
        ContextEnd::Cancelled => "confirmation cancelled by caller",
        ContextEnd::DeadlineExceeded if parent.deadline() == wait.deadline() => {
            "confirmation stopped at the caller's deadline"
        }
        ContextEnd::DeadlineExceeded => "did not settle within 10m0s",
    };
    let id = safe_text(child.id, evidence);
    let message = format!(
        "vngcloud: write outcome not confirmed: {}: {} {id} {reason}; delete was sent and a rerun is safe",
        child.operation, child.kind
    );
    let error = local(ErrorClass::NotSettled, &message, evidence);
    let error = if let Some(end) = parent.check(clock) {
        error.with_cause(context_error(end))
    } else {
        error
    };
    Err(Box::new(error))
}

fn busy(progress: &str, parent: bool) -> bool {
    matches!(progress, "CREATING" | "UPDATING" | "DELETING")
        || (parent && progress == "CREATING-BILLING")
}

fn message_has_phrase(message: &str, phrases: &[&[u8]]) -> bool {
    phrases.iter().any(|phrase| {
        message
            .as_bytes()
            .windows(phrase.len())
            .any(|part| part.eq_ignore_ascii_case(phrase))
    })
}

fn canonical_field<T: WireField>(
    fields: &Envelope<'_>,
    read: &Read,
    evidence: &Evidence,
    name: &str,
) -> Result<T, Box<Error>> {
    read.parse(fields.canonical_raw(read, evidence, name)?, evidence)
}

fn not_sent<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotSent)
}
