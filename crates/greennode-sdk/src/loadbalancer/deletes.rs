use crate::wire::product::guard::{Decision, decide};
use crate::wire::product::guard::{Guarded, guarded_poll};
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
#[derive(Clone, Default)]
pub struct DeleteLoadBalancerInput {
    /// Target load balancer identifier.
    pub load_balancer_id: String,
    /// Returns after the delete is accepted or already in progress.
    pub no_wait: bool,
}

crate::error::debug_record!(DeleteLoadBalancerInput {
    load_balancer_id,
    no_wait
});

/// A load balancer delete has no output fields.
#[derive(Clone, Default, PartialEq, Eq, Serialize)]
pub struct DeleteLoadBalancerOutput {}

crate::wire::product::output::public_fields!(DeleteLoadBalancerOutput {});

/// Identifies the certificate to delete.
#[derive(Clone, Default)]
pub struct DeleteCertificateInput {
    /// Target certificate identifier.
    pub certificate_id: String,
}

crate::error::debug_record!(DeleteCertificateInput { certificate_id });

/// A certificate delete has no output fields.
#[derive(Clone, Default, PartialEq, Eq, Serialize)]
pub struct DeleteCertificateOutput {}

crate::wire::product::output::public_fields!(DeleteCertificateOutput {});

/// Regional load balancer failure identities used in service operations.
#[derive(Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WriteError {
    /// A listener is using the pool or certificate.
    ResourceInUse,
    /// A load balancer or child did not become ready before the preflight bound.
    ResourceBusy,
}

crate::error::debug_record!(WriteError; ResourceInUse, ResourceBusy);

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
        crate::wire::product::exit::finish!(async {
            let id = input.load_balancer_id.as_str();
            let ids = [("LoadBalancerID", id)];
            validate_ids(DELETE_LB, &ids, PathIds::Strict).map_err(|error| not_sent(*error))?;
            let _lock = self.lock_load_balancer(ctx, id).await.map_err(not_sent)?;
            let mut evidence = Evidence::for_write();
            let current = self
                .load_balancer_progress(ctx, id, &mut evidence)
                .await
                .map_err(|error| not_sent(*error))?;
            let dispatch = if current
                .recheck(&evidence)
                .map_err(|error| not_sent(*error))?
                == "DELETING"
            {
                WriteDispatch::NotSent
            } else {
                self.send_delete(
                    ctx,
                    DELETE_LB,
                    LB_ROUTE,
                    id,
                    &[StatusCode::OK, StatusCode::ACCEPTED],
                    &mut evidence,
                )
                .await
                .map(drop)?;
                WriteDispatch::MayHaveBeenSent
            };
            if input.no_wait {
                return Ok(DeleteLoadBalancerOutput {});
            }

            let clock = self.session.clock();
            let settlement_deadline = clock.monotonic_now() + DELETE_BOUND;
            let wait = ctx.child_with_deadline(settlement_deadline);
            let (class, reason, cause) = guarded_poll!(evidence, {
                let poll = self.load_balancer_progress(&wait, id, &mut evidence).await;
                if let Some(end) = wait.check(clock) {
                    let reason = match end {
                        ContextEnd::Cancelled => "confirmation cancelled by caller",
                        ContextEnd::DeadlineExceeded
                            if ctx
                                .deadline()
                                .is_some_and(|deadline| deadline <= settlement_deadline) =>
                        {
                            "confirmation stopped at the caller's deadline"
                        }
                        ContextEnd::DeadlineExceeded => "did not reach 404 within 15m0s",
                    };
                    break (
                        ErrorClass::NotSettled,
                        reason.into(),
                        Some(context_error(end)),
                    );
                }
                match poll.and_then(|status| status.into_checked(&evidence)) {
                    Err(error)
                        if crate::confirms_absence(
                            &error,
                            "loadbalancer.GetLoadBalancer",
                            "GET",
                            LB_ROUTE,
                        ) =>
                    {
                        return Ok(DeleteLoadBalancerOutput {});
                    }
                    Err(error) => {
                        let reason = error
                            .api_error()
                            .map(|api| api.message())
                            .filter(|message| !message.is_empty())
                            .map_or_else(
                                || "confirmation read failed".into(),
                                |message| format!("confirmation read failed: {message}"),
                            );
                        break (ErrorClass::NotSettled, reason, Some(*error));
                    }
                    Ok(progress) => {
                        if progress == "ERROR" {
                            match current.recheck(&evidence) {
                                Ok(status) if status != "ERROR" => {
                                    break (ErrorClass::Failed, "is ERROR".into(), None);
                                }
                                Err(error) => {
                                    break (
                                        ErrorClass::NotSettled,
                                        "confirmation read failed".into(),
                                        Some(*error),
                                    );
                                }
                                Ok(_) => {}
                            }
                        }
                        wait.child_with_deadline(clock.monotonic_now() + POLL_INTERVAL)
                            .wait(clock)
                            .await;
                    }
                }
            });
            let (prefix, advice) = match (class, dispatch) {
                (ErrorClass::Failed, _) => ("vngcloud: write failed", ""),
                (_, WriteDispatch::NotSent) => (
                    "vngcloud: write outcome not confirmed",
                    "; delete was already in progress and a rerun is safe",
                ),
                _ => (
                    "vngcloud: write outcome not confirmed",
                    "; delete was sent and a rerun is safe",
                ),
            };
            let safe_id = safe_text(id, &evidence);
            let message =
                format!("{prefix}: {DELETE_LB}: load balancer {safe_id} {reason}{advice}");
            let error = local(class, &message, &evidence);
            let error = match cause {
                Some(cause) => error.with_cause(cause),
                None => error,
            };
            let partial = evidence
                .can_retain_partial()
                .then_some(DeleteLoadBalancerOutput {});
            Err(Failure::new(error, partial, dispatch))
        })
    }

    /// Deletes an unused certificate and requires a complete HTTP 204 response.
    pub async fn delete_certificate(
        &self,
        ctx: &RequestContext,
        input: DeleteCertificateInput,
    ) -> Result<DeleteCertificateOutput, Failure<DeleteCertificateOutput>> {
        crate::wire::product::exit::finish!(async {
            let id = input.certificate_id.as_str();
            validate_ids(DELETE_CERT, &[("CertificateID", id)], PathIds::Strict)
                .map_err(|error| not_sent(*error))?;
            let mut evidence = Evidence::for_write();
            if let Err(error) = self.check_certificate(ctx, id, &mut evidence).await {
                let error = if error.detail::<WriteError>().is_some() {
                    *error
                } else {
                    self.certificate_not_found(
                        ctx,
                        id,
                        *error,
                        WriteDispatch::NotSent,
                        &mut evidence,
                    )
                    .await
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
            if error.detail::<WriteError>() == Some(&WriteError::ResourceInUse) {
                let mut detail = format!(
                    "loadbalancer: certificate in use: {DELETE_CERT}: certificate {}",
                    safe_text(id, &evidence),
                );
                if let Some(api) = error.api_error() {
                    use std::fmt::Write as _;
                    let _ = write!(detail, ": {}", api.go_text());
                }
                let error = local(ErrorClass::Api, &detail, &evidence).with_cause(error);
                return Err(Failure::new(error, None, dispatch));
            }
            let error = self
                .certificate_not_found(ctx, id, error, dispatch, &mut evidence)
                .await;
            Err(Failure::new(error, None, dispatch))
        })
    }

    pub(super) async fn lock_load_balancer(
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
    ) -> Result<Guarded<String>, Box<Error>> {
        let read = Read {
            id: "loadbalancer.GetLoadBalancer",
            route: LB_ROUTE,
        };
        let text = self
            .read_guard_text(
                ctx,
                super::wire::GuardRead(&read),
                &[("LoadBalancerID", id)],
                &[],
                evidence,
            )
            .await?;
        let data = Envelope::parse(&text, &read, evidence)?;
        let body = data.canonical_raw(&read, evidence, "data")?;
        let ZonedLoadBalancer(row) =
            super::guard::parse(&read, body, &super::guard::LOAD_BALANCER, evidence)?;
        if row.uuid != id || row.progress_status.is_empty() {
            return Err(read.decode_error(evidence, DecodeError::InvalidShape));
        }
        screen_input(evidence, [&*row.uuid, &row.progress_status])?;
        Guarded::latest(row.progress_status, evidence)
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
            .read_guard_text(
                ctx,
                super::wire::GuardRead(&read),
                &[("CertificateID", id)],
                &[],
                evidence,
            )
            .await?;
        let cert: Certificate =
            super::guard::parse(&read, &text, &super::guard::CERTIFICATE, evidence)?;
        let fields = Envelope::parse(&text, &read, evidence)?;
        if cert.uuid != id || fields.canonical_raw(&read, evidence, "inUse")? == "null" {
            return Err(read.decode_error(evidence, DecodeError::InvalidShape));
        }
        screen_input(evidence, [&*cert.uuid])?;
        if cert.in_use {
            let message = format!(
                "loadbalancer: certificate in use: {DELETE_CERT}: certificate {} is in use",
                safe_text(id, evidence)
            );
            return Err(Box::new(decide(evidence, |decision| {
                resource_in_use(decision, local(ErrorClass::Api, &message, evidence))
            })?));
        }
        Ok(())
    }

    async fn certificate_not_found(
        &self,
        ctx: &RequestContext,
        id: &str,
        error: Error,
        dispatch: WriteDispatch,
        evidence: &mut Evidence,
    ) -> Error {
        use crate::wire::product::operation::RefusedGuard;
        let unresolved_404 = crate::wire::product::absence::is_not_found_diagnostic(&error);
        if crate::confirms_absence(&error, DELETE_CERT, "DELETE", CERT_ROUTE)
            || crate::confirms_absence(&error, "loadbalancer.GetCertificate", "GET", CERT_ROUTE)
            || (error.detail::<RefusedGuard>().is_some() && !unresolved_404)
            || error.has_class(ErrorClass::InvalidInput)
        {
            return evidence.screen_error(error);
        }
        let inventory = self.certificate_absent(ctx, id, evidence).await;
        if !matches!(inventory, Ok(true)) {
            if unresolved_404 {
                let class = if dispatch == WriteDispatch::MayHaveBeenSent {
                    ErrorClass::NotSettled
                } else {
                    ErrorClass::Api
                };
                let uncertain = local(
                    class,
                    "certificate absence could not be confirmed",
                    evidence,
                );
                return crate::wire::product::absence::keep_not_found_diagnostic(
                    uncertain, &error, evidence,
                );
            }
            let error = match inventory {
                Err(refusal) if refusal.detail::<RefusedGuard>().is_some() => *refusal,
                _ => error,
            };
            return evidence.screen_error(error);
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
            .read_guard_text(ctx, super::wire::GuardRead(&read), &[], &query, evidence)
            .await?;
        let text = Guarded::latest_direct(text, evidence, read.id)?.into_checked(evidence)?;
        let envelope = Envelope::parse(&text, &read, evidence)?;
        let body = envelope.canonical_raw(&read, evidence, "listData")?;
        let Some(rows): Option<Vec<Certificate>> = super::guard::parse(
            &read,
            body,
            &crate::wire::strict_json::Shape::Array(&super::guard::CERTIFICATE),
            evidence,
        )?
        else {
            return Ok(false);
        };
        let page: i64 = read.parse(envelope.canonical_raw(&read, evidence, "page")?, evidence)?;
        let size: i64 = read.parse(
            envelope.canonical_raw(&read, evidence, "pageSize")?,
            evidence,
        )?;
        let total_pages = read.parse::<Option<i64>>(
            envelope.canonical_raw(&read, evidence, "totalPage")?,
            evidence,
        )?;
        let total_items = read.parse::<Option<i64>>(
            envelope.canonical_raw(&read, evidence, "totalItem")?,
            evidence,
        )?;
        let (Some(total_pages), Some(total_items)) = (total_pages, total_items) else {
            return Ok(false);
        };
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
    ) -> Result<crate::wire::product::sender::Response, Failure<O>> {
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
            .map_err(|error| not_sent(evidence.screen_error(*error)))?;
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
            raw_body: false,
            overflow_overrides_status: false,
            sensitive: false,
            reflection: crate::wire::product::operation::Reflection::Redact,
            no_redirect: false,
            report_redirect: false,
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
        let screen = |error: Error| evidence.screen_error(error);
        let reply = result.map_err(|failure| {
            let (error, _, dispatch) = failure.into_parts();
            Failure::new(screen(error), None, dispatch)
        })?;
        let status = reply.status;
        if success.contains(&status) {
            return Ok(reply);
        }
        let absence = reply.clean_absence();
        let in_use = id == DELETE_CERT
            && reply.clean_receipt().is_some_and(|receipt| {
                response::error_message_matches(
                    receipt.body(),
                    evidence.credentials(),
                    certificate_in_use,
                )
            });
        reply
            .classify(id, success, (true, false, op.retry), false)
            .map_err(|error| {
                let mut error = screen(error.with_clean_absence(absence));
                if in_use {
                    error = decide(evidence, |decision| resource_in_use(decision, error))
                        .unwrap_or_else(|error| *error);
                }
                Failure::new(error, None, WriteDispatch::MayHaveBeenSent)
            })?;
        Ok(reply)
    }
}

pub(super) fn resource_in_use(_: Decision<'_>, error: Error) -> Error {
    error.with_outer_detail(WriteError::ResourceInUse)
}

fn certificate_in_use(message: &str) -> bool {
    [b"in use".as_slice(), b"is used"]
        .into_iter()
        .any(|needle| {
            message
                .as_bytes()
                .windows(needle.len())
                .any(|piece| piece.eq_ignore_ascii_case(needle))
        })
}

pub(super) fn context_error(end: ContextEnd) -> Error {
    Error::new(match end {
        ContextEnd::Cancelled => ErrorClass::Cancelled,
        ContextEnd::DeadlineExceeded => ErrorClass::DeadlineExceeded,
    })
}

pub(super) fn safe_text<'a>(text: &'a str, evidence: &Evidence) -> &'a str {
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

pub(super) fn local(class: ErrorClass, message: &str, evidence: &Evidence) -> Error {
    Error::new(class).with_detail(RecoveryAdvice::new(safe_text(message, evidence)))
}

fn not_sent<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotSent)
}
