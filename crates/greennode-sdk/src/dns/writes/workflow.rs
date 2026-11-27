use std::future::{Future, poll_fn};
use std::task::Poll;
use std::time::Duration;

use bytes::Bytes;
use http::{Method, StatusCode};
use serde::Serialize;
use tokio::sync::MutexGuard;

use super::super::{Client, HostedZone, Record, api_error, models::Resource, read_operation};
use super::wire::{Detail, ZoneSnapshot};
use crate::error::SafeText;
use crate::wire::product::{
    evidence::Evidence,
    guard::Guarded,
    operation::{self, GuardOperation, Operation, Output},
    options::Options,
    redirect::{Classification, Retry},
};
use crate::{Clock, ContextEnd, Error, ErrorClass, Failure, RequestContext, WriteDispatch};

pub(super) const ZONES: &str = "v1/dns/hosted-zone";
pub(super) const ZONE: &str = "v1/dns/hosted-zone/{}";
pub(super) const RECORDS: &str = "v1/dns/hosted-zone/{}/record";
pub(super) const RECORD: &str = "v1/dns/hosted-zone/{}/record/{}";
pub(super) const ACTIVE: &str = "ACTIVE";
const ERROR: &str = "ERROR";
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const POLL_BOUND: Duration = Duration::from_secs(60);

/// A local DNS write failure with a stable reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorDetail {
    /// The hosted zone did not become ready before the preflight deadline.
    ZoneBusy,
}

pub(super) struct Workflow<'a> {
    client: &'a Client,
    ctx: &'a RequestContext,
    id: &'static str,
    pub(super) evidence: Evidence,
    _guard: MutexGuard<'a, ()>,
}

#[expect(
    clippy::result_large_err,
    reason = "DNS failures carry the SDK error by value"
)]
impl<'a> Workflow<'a> {
    pub(super) async fn start<T>(
        client: &'a Client,
        ctx: &'a RequestContext,
        id: &'static str,
    ) -> Result<Self, Failure<T>> {
        let guard = until(ctx, client.session.clock(), client.write_lock.lock())
            .await
            .map_err(|end| not_sent(context_error(end)))?;
        Ok(Self {
            client,
            ctx,
            id,
            evidence: Evidence::new(),
            _guard: guard,
        })
    }

    pub(super) async fn send<T: Output>(
        &mut self,
        method: Method,
        template: &'static str,
        path: &[&str],
        body: Bytes,
    ) -> Result<(StatusCode, T), Failure<T>> {
        let acknowledgement = method != Method::POST;
        let operation = Operation {
            method,
            class: Classification::Write,
            retry: Retry::Once,
            success: if acknowledgement {
                &[StatusCode::NO_CONTENT]
            } else {
                &[StatusCode::OK]
            },
            allow_recovery: false,
            keep_status: self.id == super::UPDATE_ZONE,
            no_output: acknowledgement,
            no_redirect: true,
            ..read_operation(self.id, template)
        };
        self.call(self.ctx, &operation, path, body).await
    }

    async fn call<T: Output>(
        &mut self,
        ctx: &RequestContext,
        operation: &Operation,
        path: &[&str],
        body: Bytes,
    ) -> Result<(StatusCode, T), Failure<T>> {
        let local_failure = |error| {
            Failure::new(
                error,
                None,
                if matches!(operation.class, Classification::Write) {
                    WriteDispatch::NotSent
                } else {
                    WriteDispatch::NotAWrite
                },
            )
        };
        let session = &self.client.session;
        let base = session.endpoints().dns.as_ref().ok_or_else(|| {
            local_failure(Error::new(ErrorClass::InvalidConfig).with_cause(api_error(
                "",
                "InvalidConfig",
                "vngcloud: invalid config: DNS endpoint is unavailable",
            )))
        })?;
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(ctx, &mut jitter)
            .map_err(|error| local_failure(*error))?;
        operation::execute_with_evidence(
            runtime,
            base,
            operation,
            &Options::DEFAULT,
            path,
            &[],
            session.bearer_source(),
            body,
            &mut self.evidence,
        )
        .await
    }

    async fn read<T: Output>(
        &mut self,
        ctx: &RequestContext,
        template: &'static str,
        path: &[&str],
    ) -> Result<T, Box<Error>> {
        let op = GuardOperation::new(Operation {
            cap: 256 << 10,
            allow_empty: false,
            ..read_operation(self.id, template)
        });
        self.read_guard(ctx, &op, path).await
    }

    async fn read_guard<T: Output>(
        &mut self,
        ctx: &RequestContext,
        op: &GuardOperation,
        path: &[&str],
    ) -> Result<T, Box<Error>> {
        self.call(ctx, op, path, Bytes::new())
            .await
            .map(|(_, data)| data)
            .map_err(|err| Box::new(err.into_parts().0))
    }

    async fn zone(
        &mut self,
        ctx: &RequestContext,
        id: &str,
    ) -> Result<Guarded<HostedZone>, Box<Error>> {
        let data: Detail<Option<ZoneSnapshot>> = self.read(ctx, ZONE, &[id]).await?;
        let Some((zone, complete)) = data
            .data
            .map(ZoneSnapshot::into_zone)
            .filter(|(zone, _)| zone.id == id && !zone.status.is_empty())
        else {
            return Err(Box::new(api_error(
                "",
                "",
                &format!("{}: hosted zone identity or state is unproven", self.id),
            )));
        };
        if self.id == super::UPDATE_ZONE && !complete {
            return Err(Box::new(api_error(
                "",
                "",
                "dns.UpdateHostedZone: incomplete hosted zone replacement snapshot",
            )));
        }
        Guarded::latest(zone, &self.evidence)
    }

    async fn record(
        &mut self,
        ctx: &RequestContext,
        zone_id: &str,
        id: &str,
    ) -> Result<Guarded<Record>, Box<Error>> {
        let data: Detail<Option<Record>> = self.read(ctx, RECORD, &[zone_id, id]).await?;
        let record = data
            .data
            .filter(|record| {
                record.id == id && record.hosted_zone_id == zone_id && !record.status.is_empty()
            })
            .ok_or_else(|| {
                Box::new(api_error(
                    "",
                    "",
                    &format!("{}: record identity or state is unproven", self.id),
                ))
            })?;
        Guarded::latest(record, &self.evidence)
    }

    pub(super) async fn ready<T>(
        &mut self,
        zone_id: &str,
    ) -> Result<Guarded<HostedZone>, Failure<T>> {
        self.poll(
            |flow| {
                flow.diagnostic(
                    ErrorClass::Api,
                    &format!(
                        "dns: zone busy: {}: {} was not confirmed ready within 1m0s",
                        flow.id,
                        flow.zone_label(zone_id)
                    ),
                )
                .with_outer_detail(ErrorDetail::ZoneBusy)
            },
            async |flow: &mut Self, ctx: &RequestContext| {
                let zone = flow.zone(ctx, zone_id).await?;
                Ok(matches!(
                    zone.recheck(&flow.evidence)?.status.as_str(),
                    ACTIVE | ERROR
                )
                .then_some(zone))
            },
        )
        .await
        .map_err(|error| not_sent(*error))
    }

    pub(super) async fn settle_zone<O>(
        &mut self,
        id: &str,
        fallback: HostedZone,
        no_wait: bool,
        settled: impl Fn(&HostedZone) -> bool,
        output: impl Fn(HostedZone) -> O,
    ) -> Result<O, Failure<O>> {
        let resource = |flow: &Self| flow.zone_label(id);
        let mut last = fallback;
        let result = if no_wait {
            self.zone(self.ctx, id)
                .await
                .and_then(|zone| zone.into_checked(&self.evidence))
                .map(|zone| last = zone)
        } else {
            self.poll(
                |flow| flow.unconfirmed(&format!("{} was accepted; do not send the same write again", resource(flow))),
                async |flow: &mut Self, ctx: &RequestContext| {
                let zone = flow.zone(ctx, id).await?;
                last = zone.into_checked(&flow.evidence)?;
                if settled(&last) {
                    Ok(Some(()))
                } else if last.status == ERROR {
                    Err(Box::new(flow.diagnostic(ErrorClass::Failed, &format!(
                        "vngcloud: write failed: {}: {} is ERROR; every associated VPC needs Private DNS ENABLED, not ENABLING", flow.id, resource(flow),
                    ))))
                } else {
                    Ok(None)
                }
            }).await
        };
        self.finish(result, last, &resource(self), output)
    }

    pub(super) async fn settle_record<O>(
        &mut self,
        path: (&str, &str),
        fallback: Record,
        no_wait: bool,
        mut settled: impl FnMut(&Record, &HostedZone) -> bool,
        output: impl Fn(Record) -> O,
    ) -> Result<O, Failure<O>> {
        let (zone_id, id) = path;
        let resource = |flow: &Self| flow.record_label(id, (!no_wait).then_some(zone_id));
        let mut last = fallback;
        let result = if no_wait {
            self.record(self.ctx, zone_id, id)
                .await
                .and_then(|record| record.into_checked(&self.evidence))
                .map(|record| last = record)
        } else {
            self.poll(
                |flow| {
                    flow.unconfirmed(&format!(
                        "{} was accepted; do not send the same write again",
                        resource(flow)
                    ))
                },
                async |flow: &mut Self, ctx: &RequestContext| {
                    let zone = flow.zone(ctx, zone_id).await?;
                    let record = flow.record(ctx, zone_id, id).await?;
                    flow.record_settlement(zone_id, id, zone, record, &mut last, &mut settled)
                },
            )
            .await
        };
        self.finish(result, last, &resource(self), output)
    }

    fn record_settlement(
        &self,
        zone_id: &str,
        id: &str,
        zone: Guarded<HostedZone>,
        record: Guarded<Record>,
        last: &mut Record,
        settled: &mut impl FnMut(&Record, &HostedZone) -> bool,
    ) -> Result<Option<()>, Box<Error>> {
        let zone = zone.recheck(&self.evidence)?;
        *last = record.into_checked(&self.evidence)?;
        if zone.status == ERROR || last.status == ERROR {
            Err(Box::new(self.diagnostic(
                ErrorClass::Failed,
                &format!(
                    "vngcloud: write failed: {}: {} is ERROR",
                    self.id,
                    self.record_label(id, Some(zone_id))
                ),
            )))
        } else {
            Ok((zone.status == ACTIVE && settled(last, zone)).then_some(()))
        }
    }

    pub(super) async fn wait_gone<O>(
        &mut self,
        zone_id: &str,
        record_id: Option<&str>,
        output: O,
    ) -> Result<O, Failure<O>> {
        let resource = |flow: &Self| match record_id {
            Some(id) => flow.record_label(id, Some(zone_id)),
            None => flow.zone_label(zone_id),
        };
        let result = self
            .poll(
                |flow| {
                    flow.unconfirmed(&format!(
                        "{} was accepted; do not delete it again",
                        resource(flow)
                    ))
                },
                async |flow: &mut Self, ctx: &RequestContext| {
                    let read = match record_id {
                        Some(id) => flow.record(ctx, zone_id, id).await.map(drop),
                        None => flow.zone(ctx, zone_id).await.map(drop),
                    };
                    match read {
                        Ok(()) => Ok(None),
                        Err(error) if error.has_class(ErrorClass::NotFound) => Ok(Some(())),
                        Err(error) => Err(error),
                    }
                },
            )
            .await;
        match result {
            Ok(()) => Ok(output),
            Err(error) => {
                Err(self.after_write(self.wrap_outcome(*error, &resource(self)), || output))
            }
        }
    }

    async fn poll<T>(
        &mut self,
        on_timeout: impl FnOnce(&Self) -> Error,
        mut step: impl AsyncFnMut(&mut Self, &RequestContext) -> Result<Option<T>, Box<Error>>,
    ) -> Result<T, Box<Error>> {
        let clock = self.client.session.clock();
        let bound = clock.monotonic_now() + POLL_BOUND;
        let child = self.ctx.child_with_deadline(bound);
        let checkpoint = self.evidence.guard_checkpoint();
        while child.check(clock).is_none() {
            self.evidence.discard_guards_after(checkpoint);
            let result = step(self, &child).await;
            if child.check(clock).is_some() {
                break;
            }
            if let Some(result) = result? {
                return Ok(result);
            }
            let wake = (clock.monotonic_now() + POLL_INTERVAL).min(bound);
            if until(&child, clock, clock.sleep_until(wake)).await.is_err() {
                break;
            }
        }
        Err(Box::new(
            self.ctx
                .check(clock)
                .map_or_else(|| on_timeout(self), context_error),
        ))
    }

    pub(super) fn clean<T: Resource>(&self, mut resource: T) -> T {
        resource.redact_ids(|id| self.can_show_id(id));
        resource.redact(self.evidence.credentials());
        resource
    }

    fn finish<T: Resource, O>(
        &self,
        result: Result<(), Box<Error>>,
        resource: T,
        label: &str,
        output: impl Fn(T) -> O,
    ) -> Result<O, Failure<O>> {
        match result {
            Ok(()) => Ok(output(self.clean(resource))),
            Err(error) => Err(self.after_write(self.wrap_outcome(*error, label), || {
                output(self.clean(resource))
            })),
        }
    }

    pub(super) fn after_write<O>(&self, error: Error, output: impl FnOnce() -> O) -> Failure<O> {
        Failure::new(
            error,
            self.evidence.can_retain_partial().then(output),
            WriteDispatch::MayHaveBeenSent,
        )
    }

    pub(super) fn unconfirmed(&self, message: &str) -> Error {
        self.diagnostic(
            ErrorClass::NotSettled,
            &format!(
                "vngcloud: write outcome not confirmed: {}: {message}",
                self.id
            ),
        )
    }

    fn wrap_outcome(&self, error: Error, resource: &str) -> Error {
        if matches!(error.class(), ErrorClass::Failed | ErrorClass::NotSettled) {
            return error;
        }
        self.unconfirmed(&format!("{resource}: {}", failure_reason(&error)))
            .with_cause(error)
    }

    fn diagnostic(&self, class: ErrorClass, message: &str) -> Error {
        let secrets: Vec<_> = self.evidence.credentials().values().iter().collect();
        let message = SafeText::new(message, &secrets);
        let safe = if message.as_str().is_empty() {
            SafeText::new("DNS write outcome could not be confirmed", &[])
        } else {
            message
        };
        Error::new(class).with_detail(safe.as_str().to_owned())
    }

    fn zone_label(&self, id: &str) -> String {
        let mut label = String::from("hosted zone");
        self.append_id(&mut label, id);
        label
    }

    fn record_label(&self, record_id: &str, zone_id: Option<&str>) -> String {
        let mut label = String::from("record");
        self.append_id(&mut label, record_id);
        if let Some(zone_id) = zone_id {
            label.push_str(" in hosted zone");
            self.append_id(&mut label, zone_id);
        }
        label
    }

    fn append_id(&self, label: &mut String, id: &str) {
        if self.can_show_id(id) {
            label.push(' ');
            label.push_str(id);
        }
    }

    fn can_show_id(&self, id: &str) -> bool {
        self.evidence.can_retain_partial()
            && Options::DEFAULT.path_ids.allows(id)
            && !self.evidence.credentials().reflects(id.as_bytes())
    }
}

pub(super) fn not_sent<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotSent)
}

pub(super) fn discard_partial<T, U>(failure: Failure<T>) -> Failure<U> {
    let (error, _, dispatch) = failure.into_parts();
    Failure::new(error, None, dispatch)
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
pub(super) fn encode<T>(body: &impl Serialize) -> Result<Bytes, Failure<T>> {
    serde_json::to_vec(body)
        .map(Bytes::from)
        .map_err(|_| not_sent(invalid("DNS write body could not be encoded")))
}

pub(super) fn invalid(message: &str) -> Error {
    Error::new(ErrorClass::InvalidInput).with_cause(api_error("", "InvalidUsage", message))
}

pub(super) fn failure_reason(error: &Error) -> String {
    use std::fmt::Write as _;

    let reason = match error.class() {
        ErrorClass::Cancelled => "context canceled",
        ErrorClass::DeadlineExceeded => "context deadline exceeded",
        _ => "request failed",
    };
    let Some(api) = error.api_error() else {
        return reason.to_owned();
    };
    let mut message = String::new();
    if !api.operation().is_empty() {
        let _ = write!(message, "{}: ", api.operation());
    }
    message.push_str(if api.message().is_empty() {
        reason
    } else {
        api.message()
    });
    if let Some(status) = api.status() {
        let _ = write!(message, " (status {status})");
    }
    message
}

fn context_error(end: ContextEnd) -> Error {
    let message = match end {
        ContextEnd::Cancelled => "context canceled",
        ContextEnd::DeadlineExceeded => "context deadline exceeded",
    };
    Error::from(end).with_cause(api_error("", "", message))
}

async fn until<T>(
    ctx: &RequestContext,
    clock: &dyn Clock,
    work: impl Future<Output = T>,
) -> Result<T, ContextEnd> {
    let mut ended = std::pin::pin!(ctx.wait(clock));
    let mut work = std::pin::pin!(work);
    poll_fn(|cx| {
        if let Poll::Ready(end) = ended.as_mut().poll(cx) {
            return Poll::Ready(Err(end));
        }
        work.as_mut()
            .poll(cx)
            .map(|value| ctx.check(clock).map_or(Ok(value), Err))
    })
    .await
}
