use std::{
    collections::{BTreeMap, BTreeSet},
    future::{Future, poll_fn},
    sync::{Arc, Mutex, Weak},
    task::Poll,
    time::Duration,
};

use bytes::Bytes;
use http::{Method, StatusCode};
use serde::Serialize;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, RouteAuth},
    options::{Options, PathIds},
    redirect::{Classification, Retry},
    response, retry,
};
use crate::{
    ContextEnd, DEFAULT_PAGE_SIZE, Error, ErrorClass, Failure, RecoveryAdvice, RequestContext,
    WriteDispatch,
};

use super::wire::{DecodeError, Envelope, Read, screen_input, validate_ids};
use super::{Certificate, Client, ZonedLoadBalancer, page_query};

const DELETE_LB: &str = "loadbalancer.DeleteLoadBalancer";
const DELETE_CERT: &str = "loadbalancer.DeleteCertificate";
const LB_ROUTE: &str = "v2/{}/loadBalancers/{}";
const CERT_ROUTE: &str = "v2/{}/cas/{}";
const POLL_INTERVAL: Duration = Duration::from_secs(10);
const DELETE_BOUND: Duration = Duration::from_secs(15 * 60);

/// Identifies the load balancer to delete and controls confirmation.
#[derive(Clone, Debug, Default)]
pub struct DeleteLoadBalancerInput {
    /// Target load balancer identifier.
    pub load_balancer_id: String,
    /// Returns after the delete is accepted or already in progress.
    pub no_wait: bool,
}

/// A load balancer delete has no output fields.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DeleteLoadBalancerOutput {}

/// Identifies the certificate to delete.
#[derive(Clone, Debug, Default)]
pub struct DeleteCertificateInput {
    /// Target certificate identifier.
    pub certificate_id: String,
}

/// A certificate delete has no output fields.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DeleteCertificateOutput {}

/// Regional load balancer failure identities used in service operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WriteError {
    /// A listener is using the certificate.
    ResourceInUse,
}

#[derive(Default)]
pub(super) struct WriteLocks(Mutex<BTreeMap<String, Weak<AsyncMutex<()>>>>);

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Deletes the load balancer and waits for its detail endpoint to return 404.
    pub async fn delete_load_balancer(
        &self,
        ctx: &RequestContext,
        input: DeleteLoadBalancerInput,
    ) -> Result<DeleteLoadBalancerOutput, Failure<DeleteLoadBalancerOutput>> {
        let id = input.load_balancer_id.as_str();
        let ids = [("LoadBalancerID", id)];
        validate_ids(DELETE_LB, &ids, PathIds::Strict).map_err(|error| not_sent(*error))?;
        let _lock = self.lock_load_balancer(ctx, id).await.map_err(not_sent)?;
        let mut evidence = Evidence::new();
        let current = self
            .load_balancer_progress(ctx, id, &mut evidence)
            .await
            .map_err(|error| not_sent(*error))?;
        if current != "DELETING" {
            self.send_delete(
                ctx,
                DELETE_LB,
                LB_ROUTE,
                id,
                &[StatusCode::OK, StatusCode::ACCEPTED],
                &mut evidence,
            )
            .await?;
        }
        if input.no_wait {
            return Ok(DeleteLoadBalancerOutput {});
        }

        let clock = self.session.clock();
        let wait = ctx.child_with_deadline(clock.monotonic_now() + DELETE_BOUND);
        let was_error = current == "ERROR";
        let (class, reason, cause) = loop {
            if let Some(end) = wait.check(clock) {
                break (
                    ErrorClass::NotSettled,
                    "did not reach 404 within 15m0s; delete was sent and a rerun is safe",
                    Some(context_error(end)),
                );
            }
            match self.load_balancer_progress(&wait, id, &mut evidence).await {
                Err(error)
                    if complete_not_found(&error, "loadbalancer.GetLoadBalancer")
                        && wait.check(clock).is_none() =>
                {
                    return Ok(DeleteLoadBalancerOutput {});
                }
                Ok(progress) if progress == "ERROR" && !was_error => {
                    break (ErrorClass::Failed, "is ERROR", None);
                }
                Err(error) => {
                    break (
                        ErrorClass::NotSettled,
                        "delete was sent and a rerun is safe",
                        Some(*error),
                    );
                }
                Ok(_) => {
                    wait.child_with_deadline(clock.monotonic_now() + POLL_INTERVAL)
                        .wait(clock)
                        .await;
                }
            }
        };
        let prefix = if class == ErrorClass::Failed {
            "vngcloud: write failed"
        } else {
            "vngcloud: write outcome not confirmed"
        };
        let safe_id = safe_text(id, &evidence);
        let message = format!("{prefix}: {DELETE_LB}: load balancer {safe_id} {reason}");
        let error = local(class, &message, &evidence);
        let error = match cause {
            Some(cause) => error.with_cause(cause),
            None => error,
        };
        let partial = evidence
            .can_retain_partial()
            .then_some(DeleteLoadBalancerOutput {});
        Err(Failure::new(error, partial, WriteDispatch::MayHaveBeenSent))
    }

    /// Deletes an unused certificate and requires a complete HTTP 204 response.
    pub async fn delete_certificate(
        &self,
        ctx: &RequestContext,
        input: DeleteCertificateInput,
    ) -> Result<DeleteCertificateOutput, Failure<DeleteCertificateOutput>> {
        let id = input.certificate_id.as_str();
        validate_ids(DELETE_CERT, &[("CertificateID", id)], PathIds::Strict)
            .map_err(|error| not_sent(*error))?;
        let mut evidence = Evidence::new();
        if let Err(error) = self.check_certificate(ctx, id, &mut evidence).await {
            let error = if error.detail::<WriteError>().is_none()
                && error.detail::<DecodeError>().is_none()
            {
                self.certificate_not_found(ctx, id, *error, &mut evidence)
                    .await
            } else {
                *error
            };
            return Err(not_sent(error));
        }
        let result = self
            .send_delete::<()>(
                ctx,
                DELETE_CERT,
                CERT_ROUTE,
                id,
                &[StatusCode::NO_CONTENT],
                &mut evidence,
            )
            .await;
        let Err(failure) = result else {
            return Ok(DeleteCertificateOutput {});
        };
        let (error, _, dispatch) = failure.into_parts();
        if let Some(api) = error.api_error() {
            let normalized = api.message().to_ascii_lowercase();
            if normalized.contains("in use") || normalized.contains("is used") {
                let detail = format!(
                    "loadbalancer: certificate in use: {DELETE_CERT}: certificate {}: {DELETE_CERT}: {} (status {})",
                    safe_text(id, &evidence),
                    api.message(),
                    api.status()
                        .map_or_else(|| "withheld".into(), |status| status.to_string()),
                );
                let error = local(ErrorClass::Api, &detail, &evidence)
                    .with_outer_detail(WriteError::ResourceInUse)
                    .with_cause(error);
                return Err(Failure::new(error, None, dispatch));
            }
        }
        let error = self
            .certificate_not_found(ctx, id, error, &mut evidence)
            .await;
        Err(Failure::new(error, None, dispatch))
    }

    async fn lock_load_balancer(
        &self,
        ctx: &RequestContext,
        id: &str,
    ) -> Result<OwnedMutexGuard<()>, Error> {
        let lock = {
            let mut locks = self
                .writes
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            locks.retain(|_, lock| lock.strong_count() > 0);
            if let Some(lock) = locks.get(id).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(AsyncMutex::new(()));
                locks.insert(id.to_owned(), Arc::downgrade(&lock));
                lock
            }
        };
        let clock = self.session.clock();
        let mut ended = std::pin::pin!(ctx.wait(clock));
        let mut locking = std::pin::pin!(lock.lock_owned());
        poll_fn(|cx| {
            if let Poll::Ready(end) = ended.as_mut().poll(cx) {
                return Poll::Ready(Err(context_error(end)));
            }
            locking.as_mut().poll(cx).map(Ok)
        })
        .await
    }

    async fn load_balancer_progress(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<String, Box<Error>> {
        let read = Read {
            id: "loadbalancer.GetLoadBalancer",
            route: LB_ROUTE,
        };
        let text = self
            .read_text(ctx, &read, &[("LoadBalancerID", id)], &[], evidence, true)
            .await?;
        let data = Envelope::parse(&text, &read, evidence)?;
        let ZonedLoadBalancer(row) = read.parse(data.raw(&read, evidence, "data")?, evidence)?;
        if row.uuid != id || row.progress_status.is_empty() {
            return Err(read.decode_error(evidence, DecodeError::InvalidShape));
        }
        screen_input(evidence, [&*row.uuid, &row.progress_status])?;
        Ok(row.progress_status)
    }

    async fn check_certificate(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<(), Box<Error>> {
        let read = Read {
            id: "loadbalancer.GetCertificate",
            route: CERT_ROUTE,
        };
        let text = self
            .read_text(ctx, &read, &[("CertificateID", id)], &[], evidence, true)
            .await?;
        let cert: Certificate = read.parse(&text, evidence)?;
        let fields = Envelope::parse(&text, &read, evidence)?;
        if cert.uuid != id || fields.raw(&read, evidence, "inUse")? == "null" {
            return Err(read.decode_error(evidence, DecodeError::InvalidShape));
        }
        screen_input(evidence, [&*cert.uuid])?;
        if cert.in_use {
            let message = format!(
                "loadbalancer: certificate in use: {DELETE_CERT}: certificate {} is in use",
                safe_text(id, evidence)
            );
            return Err(Box::new(
                local(ErrorClass::Api, &message, evidence)
                    .with_outer_detail(WriteError::ResourceInUse),
            ));
        }
        Ok(())
    }

    async fn certificate_not_found(
        &self,
        ctx: &RequestContext,
        id: &str,
        error: Error,
        evidence: &mut Evidence,
    ) -> Error {
        if complete_not_found(&error, "loadbalancer.GetCertificate")
            || complete_not_found(&error, DELETE_CERT)
            || error.has_class(ErrorClass::InvalidInput)
            || !self
                .certificate_absent(ctx, id, evidence)
                .await
                .unwrap_or(false)
        {
            return error.screen_api(&|value| evidence.text_reflects(value));
        }
        let message = format!(
            "vngcloud: resource not found: {DELETE_CERT}: certificate {}",
            safe_text(id, evidence)
        );
        Error::new(ErrorClass::NotFound)
            .with_detail(RecoveryAdvice::new(safe_text(&message, evidence)))
    }

    async fn certificate_absent(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<bool, Box<Error>> {
        let read = Read {
            id: "loadbalancer.ListCertificates",
            route: "v2/{}/cas",
        };
        let query = page_query("", 0, DEFAULT_PAGE_SIZE);
        let query = query.each_ref().map(|(key, value)| (*key, value.as_str()));
        let text = self
            .read_text(ctx, &read, &[], &query, evidence, true)
            .await?;
        let envelope = Envelope::parse(&text, &read, evidence)?;
        let Some(rows) = read.parse::<Option<Vec<Certificate>>>(
            envelope.raw(&read, evidence, "listData")?,
            evidence,
        )?
        else {
            return Ok(false);
        };
        let page: i64 = envelope.field(&read, evidence, "page")?;
        let size: i64 = envelope.field(&read, evidence, "pageSize")?;
        let total_pages: i64 = envelope.field(&read, evidence, "totalPage")?;
        let total_items: i64 = envelope.field(&read, evidence, "totalItem")?;
        let count = i64::try_from(rows.len()).unwrap_or(i64::MAX);
        let mut seen = BTreeSet::new();
        Ok(page == 1
            && size >= DEFAULT_PAGE_SIZE
            && matches!(total_pages, 0 | 1)
            && count <= size
            && total_items == count
            && (total_pages == 1 || count == 0)
            && rows.iter().all(|row| {
                row.uuid != id
                    && PathIds::WithUnderscore.allows(&row.uuid)
                    && !evidence.text_reflects(&row.uuid)
                    && seen.insert(&row.uuid)
            }))
    }

    async fn send_delete<O>(
        &self,
        ctx: &RequestContext,
        id: &'static str,
        route: &'static str,
        resource: &str,
        success: &'static [StatusCode],
        evidence: &mut Evidence,
    ) -> Result<(), Failure<O>> {
        let session = &self.session;
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(ctx, &mut jitter)
            .map_err(|error| not_sent(*error))?;
        if let Some(credentials) = runtime.configured_credentials {
            evidence.absorb(credentials);
        }
        screen_input(evidence, [resource]).map_err(|error| not_sent(*error))?;
        let project = session
            .require_project_id_with_evidence(ctx, evidence)
            .await
            .map_err(|error| not_sent(error.screen_api(&|value| evidence.text_reflects(value))))?;
        let path = [project.as_str(), resource];
        screen_input(evidence, path).map_err(|error| not_sent(*error))?;
        let base = session
            .endpoints()
            .vlb
            .as_ref()
            .ok_or_else(|| not_sent(Error::new(ErrorClass::InvalidConfig)))?;
        let op = Operation {
            id,
            template: route,
            method: Method::DELETE,
            class: Classification::Write,
            retry: Retry::Repeatable,
            success,
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
            &op,
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
        response::classify(
            id,
            reply.status,
            Bytes::from(reply.reply.body),
            success,
            (true, false, op.retry),
            evidence.credentials(),
        )
        .map(|_| ())
        .map_err(|error| Failure::new(screen(*error), None, WriteDispatch::MayHaveBeenSent))
    }
}

fn complete_not_found(error: &Error, operation: &str) -> bool {
    !error.is_body_too_large()
        && error
            .api_error()
            .is_some_and(|api| api.operation() == operation && api.status() == Some(404))
}

fn context_error(end: ContextEnd) -> Error {
    Error::new(match end {
        ContextEnd::Cancelled => ErrorClass::Cancelled,
        ContextEnd::DeadlineExceeded => ErrorClass::DeadlineExceeded,
    })
}

fn safe_text<'a>(text: &'a str, evidence: &Evidence) -> &'a str {
    if evidence.text_reflects(text) {
        if evidence.text_reflects("<redacted>") {
            ""
        } else {
            "<redacted>"
        }
    } else {
        text
    }
}

fn local(class: ErrorClass, message: &str, evidence: &Evidence) -> Error {
    Error::new(class).with_detail(RecoveryAdvice::new(safe_text(message, evidence)))
}

fn not_sent<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotSent)
}
