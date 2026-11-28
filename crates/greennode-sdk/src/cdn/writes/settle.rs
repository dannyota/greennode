use crate::wire::product::guard::guarded_poll;
use crate::wire::product::operation::RefusedGuard;
use std::time::Duration;

use bytes::Bytes;
use tokio::time::Instant;

use super::{Call, Client, WebDetail, WriteKind, outcome, redact, rejected, safe_status};
use crate::cdn::{ErrorDetail, WebAccelerator, wire::convert};
use crate::wire::product::evidence::Evidence;
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};

const BOUND: Duration = Duration::from_secs(360);
const INTERVAL: Duration = Duration::from_secs(10);
const CONFIRM_OFFSETS: [u64; 4] = [0, 2, 4, 8];
const SENT: WriteDispatch = WriteDispatch::MayHaveBeenSent;

#[derive(Clone, Copy)]
pub(super) struct Target {
    pending: i64,
    settled: i64,
}

impl Target {
    pub(super) const ACTIVE: Self = Self {
        pending: 3,
        settled: 1,
    };
    const DISABLED: Self = Self {
        pending: 5,
        settled: 0,
    };
}

pub(super) struct Poll<'a> {
    client: &'a Client,
    parent: &'a RequestContext,
    context: RequestContext,
    id: &'static str,
    cdn_id: &'a str,
    started: Instant,
    deadline: Instant,
    pub(super) evidence: &'a mut Evidence,
}

impl<'a> Poll<'a> {
    pub(super) fn new(
        client: &'a Client,
        ctx: &'a RequestContext,
        call: &Call,
        cdn_id: &'a str,
        evidence: &'a mut Evidence,
    ) -> Self {
        let started = client.session.clock().monotonic_now();
        let deadline = started + BOUND;
        Self {
            client,
            parent: ctx,
            context: ctx.child_with_deadline(deadline),
            id: call.id,
            cdn_id,
            started,
            deadline,
            evidence,
        }
    }

    async fn wait(&self, until: Instant) -> Result<(), Error> {
        let clock = self.client.session.clock();
        self.context.child_with_deadline(until).wait(clock).await;
        self.context
            .check(clock)
            .map_or(Ok(()), |end| Err(end.into()))
    }

    async fn detail(&mut self) -> Result<WebDetail, Error> {
        let result = self
            .client
            .read_guard_detail(&self.context, self.id, self.cdn_id, self.evidence)
            .await;
        if self.client.session.clock().monotonic_now() >= self.deadline {
            return Err(Error::new(ErrorClass::DeadlineExceeded));
        }
        result
            .map_err(|failure| failure.into_parts().0)
            .and_then(|detail| detail.into_checked(self.evidence).map_err(|error| *error))
    }

    async fn confirm(&mut self, target: Target) -> Result<WebDetail, Option<Error>> {
        let mut last = None;
        guarded_poll!(self.evidence, for seconds in CONFIRM_OFFSETS, {
            let at = self.started + Duration::from_secs(seconds);
            if self.client.session.clock().monotonic_now() < at {
                self.wait(at).await.map_err(Some)?;
            }
            match self.detail().await {
                Ok(detail) if [target.pending, target.settled].contains(&detail.status) => {
                    return Ok(detail);
                }
                Ok(_) => last = None,
                Err(error) if error.detail::<RefusedGuard>().is_some() => return Err(Some(error)),
                Err(error) => last = Some(error),
            }
        });
        Err(last)
    }

    pub(super) async fn read_once(&mut self, last: WebDetail) -> (WebDetail, Option<Error>) {
        match self.detail().await {
            Ok(detail) => (detail, None),
            Err(cause) => {
                let error = self.not_settled(&last, Some(cause));
                (last, Some(error))
            }
        }
    }

    pub(super) async fn settle(
        &mut self,
        target: Target,
        mut last: WebDetail,
        already_read: bool,
    ) -> (WebDetail, Option<Error>) {
        let mut observed = already_read;
        guarded_poll!(self.evidence, {
            let mut cause = None;
            if !observed {
                match self.detail().await {
                    Ok(detail) => {
                        last = detail;
                        observed = true;
                    }
                    Err(error) if crate::cdn::wire::confirms_missing_detail(&error, self.id) => {
                        return (last, Some(error));
                    }
                    Err(error) if error.detail::<RefusedGuard>().is_some() => {
                        let error = self.not_settled(&last, Some(error));
                        return (last, Some(error));
                    }
                    Err(error) => cause = Some(error),
                }
            }
            if observed {
                if last.status == target.settled {
                    return (last, None);
                }
                if last.status != target.pending {
                    let error = outcome(
                        ErrorDetail::UnexpectedStatus,
                        format!(
                            "cdn: unexpected CDN status: {}: the CDN went to {} while waiting for {}",
                            self.id,
                            safe_status(last.status, self.evidence),
                            safe_status(target.settled, self.evidence)
                        ),
                    );
                    return (last, Some(error));
                }
            }
            let clock = self.client.session.clock();
            let now = clock.monotonic_now();
            let parent_end = self.parent.check(clock);
            if let Some(end) = parent_end {
                cause = Some(end.into());
            }
            if now >= self.deadline || parent_end.is_some() {
                let error = self.not_settled(&last, cause);
                return (last, Some(error));
            }
            if let Err(end) = self.wait((now + INTERVAL).min(self.deadline)).await {
                let error = self.not_settled(&last, cause.or(Some(end)));
                return (last, Some(error));
            }
            observed = false;
        });
    }

    fn not_settled(&self, last: &WebDetail, cause: Option<Error>) -> Error {
        let message = format!(
            "{}: the CDN is not settled (last seen {}) but the server accepted the write; do not repeat it, read the CDN later",
            self.id,
            safe_status(last.status, self.evidence)
        );
        let error = Error::new(ErrorClass::NotSettled).with_detail(message);
        match cause {
            Some(cause) => error.with_cause(cause),
            None => error,
        }
    }
}

impl Client {
    pub(super) async fn toggle<T>(
        &self,
        ctx: &RequestContext,
        call: &Call,
        cdn_id: &str,
        kind: WriteKind,
        no_wait: bool,
        output: impl FnOnce(WebAccelerator, bool) -> T,
    ) -> Result<T, Failure<T>> {
        let mut evidence = self
            .write_evidence(ctx, None, Some(cdn_id))
            .map_err(convert)?;
        let (initial, send) = self
            .preflight(ctx, call, cdn_id, kind, &mut evidence)
            .await
            .map_err(convert)?;
        if !send {
            return Ok(output(redact(initial, &evidence), false));
        }
        let write_error = match self
            .call_with_evidence(ctx, call, &[cdn_id], Bytes::new(), &mut evidence)
            .await
        {
            Ok(_) => None,
            Err(failure) if failure.error().detail::<RefusedGuard>().is_some() => {
                return Err(convert(failure));
            }
            Err(failure) if rejected(&failure) => {
                return Err(convert(
                    self.explain_401(ctx, call, cdn_id, failure, &mut evidence)
                        .await,
                ));
            }
            Err(failure) => Some(failure.into_parts().0),
        };
        let target = if kind == WriteKind::Disable {
            Target::DISABLED
        } else {
            Target::ACTIVE
        };
        let mut poll = Poll::new(self, ctx, call, cdn_id, &mut evidence);
        let confirmed = match poll.confirm(target).await {
            Ok(detail) => detail,
            Err(read_error) => {
                let mut error =
                    Error::new(ErrorClass::Api).with_detail(ErrorDetail::StatusUnconfirmed);
                for cause in [write_error, read_error].into_iter().flatten() {
                    error = error.with_cause(cause);
                }
                return Err(Failure::new(error, None, SENT));
            }
        };
        if no_wait {
            return Ok(output(redact(confirmed, &evidence), true));
        }
        let (last, error) = poll.settle(target, confirmed, true).await;
        finish(last, error, &evidence, |model| output(model, true))
    }
}

pub(super) fn finish<T>(
    detail: WebDetail,
    error: Option<Error>,
    evidence: &Evidence,
    output: impl FnOnce(WebAccelerator) -> T,
) -> Result<T, Failure<T>> {
    let output = output(redact(detail, evidence));
    match error {
        Some(error) => Err(Failure::new(error, Some(output), SENT)),
        None => Ok(output),
    }
}
