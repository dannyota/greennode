use super::Client;
use crate::config::session::AuthKind;
use crate::wire::product::guard::{Guarded, guarded_poll};
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Output},
    options::Options,
};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Clock, Error, ErrorClass, Failure, RequestContext, WriteDispatch};
use serde::Deserialize;
use std::time::Duration;

#[derive(Default, Deserialize)]
#[serde(default)]
struct ClusterState {
    id: String,
    status: String,
}

impl Output for ClusterState {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        ShapeField {
            name: "id",
            shape: Shape::Any,
        },
        ShapeField {
            name: "status",
            shape: Shape::Any,
        },
    ]);
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    pub(super) async fn cluster_state(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<Guarded<String>, Box<Error>> {
        let base = self
            .base::<()>()
            .map_err(|failure| Box::new(failure.into_parts().0))?;
        let mut jitter = || self.session.retry_jitter();
        let runtime = self.session.runtime(ctx, &mut jitter)?;
        let mut op = super::read_operation("vks.GetCluster", "v1/clusters/{}");
        op.no_redirect = true;
        op.report_redirect = true;
        let op = operation::GuardOperation::new(op);
        let (_, state) = op
            .execute::<ClusterState>(
                runtime,
                base,
                &Options::DEFAULT,
                &[id],
                &[],
                self.session.bearer_source(),
                evidence,
            )
            .await
            .map_err(|failure| Box::new(super::safe_error(op.id, failure.error())))?;
        state.try_map(evidence, |state| {
            if state.id != id || !valid_status(&state.status) {
                return Err(Box::new(super::malformed(op.id)));
            }
            Ok(state.status)
        })
    }

    pub(super) fn write_base<T>(&self) -> Result<&crate::endpoint::Base, Failure<T>> {
        let session = &self.session;
        let base = self
            .base::<T>()
            .map_err(|failure| not_sent(failure.into_parts().0))?;
        if session.auth_kind() != AuthKind::IamUser
            || !session.uses_default_auth_endpoints()
            || session
                .endpoints()
                .vks
                .as_ref()
                .map(|base| base.configured_url())
                != session
                    .default_endpoints()
                    .vks
                    .as_ref()
                    .map(|base| base.configured_url())
        {
            return Err(Failure::new(
                super::local_error(
                    ErrorClass::InvalidConfig,
                    "InvalidConfig",
                    "greennode: invalid config: VKS writes require SDK IAM-user login and default endpoints",
                ),
                None,
                WriteDispatch::NotSent,
            ));
        }
        Ok(base)
    }
}

pub(super) fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

pub(super) fn valid_status(value: &str) -> bool {
    value.len() <= 64
        && value.bytes().next().is_some_and(|b| b.is_ascii_uppercase())
        && value
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

pub(super) fn not_sent<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotSent)
}

pub(super) async fn wait_cluster(
    ctx: &RequestContext,
    clock: &dyn Clock,
    evidence: &mut Evidence,
    mut read: impl AsyncFnMut(&RequestContext, &mut Evidence) -> Result<bool, Box<Error>>,
) -> Result<(), Box<Error>> {
    let deadline = clock.monotonic_now() + Duration::from_secs(30 * 60);
    let wait = ctx.child_with_deadline(deadline);
    guarded_poll!(evidence, {
        if let Some(end) = wait.check(clock) {
            return Err(Box::new(end.into()));
        }
        let result = read(&wait, evidence).await;
        if let Some(end) = wait.check(clock) {
            return Err(Box::new(end.into()));
        }
        if result? {
            return Ok(());
        }
        let until = (clock.monotonic_now() + Duration::from_secs(10)).min(deadline);
        let mut end = std::pin::pin!(wait.wait(clock));
        let mut sleep = clock.sleep_until(until);
        let stopped = std::future::poll_fn(|cx| {
            use std::future::Future as _;
            if let std::task::Poll::Ready(end) = end.as_mut().poll(cx) {
                return std::task::Poll::Ready(Some(end));
            }
            if sleep.as_mut().poll(cx).is_ready() {
                return std::task::Poll::Ready(None);
            }
            std::task::Poll::Pending
        })
        .await;
        if let Some(end) = stopped {
            return Err(Box::new(end.into()));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn vks_polls_thirty_minutes_with_one_mib_receipts() {
        use crate::TestClock;
        let clock = TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
        let start = clock.monotonic_now();
        let ctx = RequestContext::new().0;
        let mut evidence = Evidence::new();
        let mut body = br#"{"id":"cluster-1","status":"CREATING"}"#.to_vec();
        body.resize(1 << 20, b' ');
        let completed = std::cell::Cell::new(0);
        let result = tokio::task::unconstrained(wait_cluster(
            &ctx,
            &clock,
            &mut evidence,
            async |_, evidence| {
                if !evidence.keep_guard_for_test("vks.GetCluster", &body) {
                    return Err(Box::new(Error::new(ErrorClass::Api)));
                }
                completed.set(completed.get() + 1);
                Ok(false)
            },
        ));
        tokio::pin!(result);
        for tick in 0..180 {
            assert!(
                std::future::poll_fn(|cx| std::task::Poll::Ready(
                    std::future::Future::poll(result.as_mut(), cx).is_pending()
                ))
                .await,
                "poll exited at tick {tick}"
            );
            clock.advance(Duration::from_secs(10));
        }
        assert_eq!(
            result.await.unwrap_err().class(),
            ErrorClass::DeadlineExceeded
        );
        assert_eq!(completed.get(), 180);
        assert_eq!(
            clock.monotonic_now().duration_since(start),
            Duration::from_secs(1800)
        );
    }
}
