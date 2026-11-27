use super::Client;
use crate::config::session::AuthKind;
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
    ) -> Result<String, Box<Error>> {
        let base = self
            .base::<()>()
            .map_err(|failure| Box::new(failure.into_parts().0))?;
        let mut jitter = || self.session.retry_jitter();
        let runtime = self.session.runtime(ctx, &mut jitter)?;
        let mut op = super::read_operation("vks.GetCluster", "v1/clusters/{}");
        op.no_redirect = true;
        op.keep_status = true;
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
        let state = state.into_checked(evidence)?;
        if state.id != id || !valid_status(&state.status) {
            return Err(Box::new(super::malformed(op.id)));
        }
        Ok(state.status)
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
    mut read: impl AsyncFnMut(&RequestContext) -> Result<bool, Box<Error>>,
) -> Result<(), Box<Error>> {
    let deadline = clock.monotonic_now() + Duration::from_secs(30 * 60);
    let wait = ctx.child_with_deadline(deadline);
    loop {
        if let Some(end) = wait.check(clock) {
            return Err(Box::new(end.into()));
        }
        let result = read(&wait).await;
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
    }
}
