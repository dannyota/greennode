use super::*;
use crate::wire::product::guard::guarded_poll;
use serde_json::json;

const DELETE_POOL: &str = "globalloadbalancer.DeletePool";
const DELETE_LISTENER: &str = "globalloadbalancer.DeleteListener";

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Deletes the only listener, verifying its parent, pool, and member group survive.
    pub async fn delete_listener(
        &self,
        ctx: &RequestContext,
        input: DeleteListenerInput,
    ) -> Result<DeleteListenerOutput, Failure<DeleteListenerOutput>> {
        crate::wire::product::exit::finish!(async {
            let (glb, id) = (input.load_balancer_id.as_str(), input.listener_id.as_str());
            ids(
                DELETE_LISTENER,
                &[("LoadBalancerID", glb), ("ListenerID", id)],
            )
            .map_err(preflight)?;
            let mut run = self.start_child(DELETE_LISTENER)?;
            run.screen_input(ctx, &json!([glb, id]))
                .map_err(preflight)?;
            let parent = run.parent(ctx, glb).await.map_err(preflight)?;
            let region = run.region(ctx, &parent).await.map_err(preflight)?;
            let listener = run
                .read_guard::<ListenerWire>(ctx, LISTENER, &[glb, id], false)
                .await
                .map_err(preflight)?;
            let listeners = run.listeners(ctx, glb).await.map_err(preflight)?;
            let row = listener.recheck(&run.0.evidence).map_err(preflight)?;
            if row.id != id
                || row.glb != glb
                || row.protocol != TCP
                || state(&row.status) != State::Active
                || !matches!(listeners.recheck(&run.0.evidence).map_err(preflight)?.as_slice(), [row] if row.id == id)
                || !PathIds::Strict.allows(&row.pool)
            {
                return Err(preflight(refuse(
                    "the listener is not an eligible sole TCP listener",
                )));
            }
            let pool = run.in_pool(ctx, glb, &row.pool).await.map_err(preflight)?;
            if !stable(pool.recheck(&run.0.evidence).map_err(preflight)?) {
                return Err(preflight(refuse("the pool is not in a stable state")));
            }
            let group = run
                .sole_group(ctx, glb, &row.pool)
                .await
                .map_err(preflight)?;
            let evidence = &run.0.evidence;
            listeners.recheck(evidence).map_err(preflight)?;
            let listener = listener.recheck(evidence).map_err(preflight)?;
            pool.recheck(evidence).map_err(preflight)?;
            let group_row = group.recheck(evidence).map_err(preflight)?;
            if group_row.region != *region.recheck(evidence).map_err(preflight)? {
                return Err(preflight(refuse(
                    "the member group is not the expected private group in the configured region",
                )));
            }
            let plan = DeletePlan {
                kind: Kind::Listener,
                glb,
                id,
                pool: &listener.pool,
                group: &group,
            };
            run.delete(ctx, plan, DeleteListenerOutput::new).await
        })
    }

    /// Deletes the listener-free pool and its one private group, then verifies their absence.
    pub async fn delete_pool(
        &self,
        ctx: &RequestContext,
        input: DeletePoolInput,
    ) -> Result<DeletePoolOutput, Failure<DeletePoolOutput>> {
        crate::wire::product::exit::finish!(async {
            let (glb, id) = (input.load_balancer_id.as_str(), input.pool_id.as_str());
            ids(DELETE_POOL, &[("LoadBalancerID", glb), ("PoolID", id)]).map_err(preflight)?;
            let mut run = self.start_child(DELETE_POOL)?;
            run.screen_input(ctx, &json!([glb, id]))
                .map_err(preflight)?;
            let parent = run.parent(ctx, glb).await.map_err(preflight)?;
            let region = run.region(ctx, &parent).await.map_err(preflight)?;
            let pool = run.in_pool(ctx, glb, id).await.map_err(preflight)?;
            if !stable(pool.recheck(&run.0.evidence).map_err(preflight)?) {
                return Err(preflight(refuse("the pool is not in a stable state")));
            }
            run.no_listeners(ctx, glb).await.map_err(preflight)?;
            let group = run.sole_group(ctx, glb, id).await.map_err(preflight)?;
            let evidence = &run.0.evidence;
            pool.recheck(evidence).map_err(preflight)?;
            let group_row = group.recheck(evidence).map_err(preflight)?;
            if group_row.region != *region.recheck(evidence).map_err(preflight)? {
                return Err(preflight(refuse(
                    "the member group is not the expected private group with one backend",
                )));
            }
            if !healthy(&group_row.status) || !member(group_row).is_some_and(|m| healthy(&m.status))
            {
                return Err(preflight(refuse(
                    "the backend row is not settled; wait for a health status",
                )));
            }
            let plan = DeletePlan {
                kind: Kind::Pool,
                glb,
                id,
                pool: id,
                group: &group,
            };
            run.delete(ctx, plan, DeletePoolOutput::new).await
        })
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Pool,
    Listener,
}

struct DeletePlan<'a> {
    kind: Kind,
    glb: &'a str,
    id: &'a str,
    pool: &'a str,
    group: &'a Guarded<Group>,
}

impl Child<'_> {
    async fn in_pool(
        &mut self,
        ctx: &RequestContext,
        glb: &str,
        id: &str,
    ) -> Result<Guarded<PoolWire>, Box<Error>> {
        let checkpoint = self.0.evidence.guard_checkpoint();
        let pools = self.pools(ctx, glb).await?;
        let pool = match pools.recheck(&self.0.evidence)?.as_slice() {
            [] => Err(Box::new(local(
                ErrorClass::NotFound,
                "NotFound",
                "vngcloud: resource not found: the load balancer holds no pool",
            ))),
            [pool] if pool.id == id => self.exact_pool(ctx, glb, id, "").await,
            _ => Err(refuse(
                "the load balancer must hold exactly the expected pool",
            )),
        }?;
        pools.recheck(&self.0.evidence)?;
        let value = pool.into_checked(&self.0.evidence)?;
        Guarded::from_checkpoint(value, &self.0.evidence, checkpoint)
    }

    async fn sole_group(
        &mut self,
        ctx: &RequestContext,
        glb: &str,
        pool: &str,
    ) -> Result<Guarded<Group>, Box<Error>> {
        let checkpoint = self.0.evidence.guard_checkpoint();
        let rows = self.groups(ctx, glb, pool).await?;
        let [row] = rows.recheck(&self.0.evidence)?.as_slice() else {
            return Err(refuse("the pool must hold exactly one member group"));
        };
        let group = self.exact_group(ctx, glb, pool, &row.id).await?;
        rows.recheck(&self.0.evidence)?;
        let value = group.into_checked(&self.0.evidence)?;
        Guarded::from_checkpoint(value, &self.0.evidence, checkpoint)
    }

    #[allow(clippy::result_large_err)]
    async fn delete<O: Serialize>(
        &mut self,
        ctx: &RequestContext,
        plan: DeletePlan<'_>,
        output: impl Fn(&str, &str, bool, &Evidence) -> O,
    ) -> Result<O, Failure<O>> {
        let result = |run: &Child<'_>, deleted| output(plan.glb, plan.id, deleted, &run.0.evidence);
        let template = match plan.kind {
            Kind::Pool => POOL,
            Kind::Listener => LISTENER,
        };
        self.send(
            ctx,
            template,
            Method::DELETE,
            &[plan.glb, plan.id],
            None,
            |run| Some(result(run, false)),
        )
        .await
        .map(drop)?;
        let wait = self.0.wait_context(ctx, DELETE_WAIT);
        let (reason, cause) = guarded_poll!(self.0.evidence, {
            if self.0.interrupted(&wait) {
                break ("delete confirmation interrupted", None);
            }
            match self.check_delete(&wait, &plan).await {
                Ok(false) => self.0.pause(&wait).await,
                Ok(true) => {
                    if self.0.interrupted(&wait) {
                        break ("delete confirmation interrupted", None);
                    }
                    let out = result(self, true);
                    if self.0.safe_output(&out) {
                        return Ok(out);
                    }
                    break ("output withheld", None);
                }
                Err(CheckFailure { reason, cause }) => break (reason, cause),
            }
        });
        let failure = self.fail(reason, result(self, false));
        let Some(cause) = cause else {
            return Err(failure);
        };
        let (error, partial, dispatch) = failure.into_parts();
        Err(Failure::new(error.with_cause(*cause), partial, dispatch))
    }

    async fn check_delete(
        &mut self,
        ctx: &RequestContext,
        plan: &DeletePlan<'_>,
    ) -> Result<bool, CheckFailure> {
        let interrupted = "delete confirmation interrupted";
        let unproven = "delete absence could not be proven";
        let reads = match plan.kind {
            Kind::Pool => {
                let rows = self
                    .pools(ctx, plan.glb)
                    .await
                    .map_err(|cause| CheckFailure {
                        reason: interrupted,
                        cause: Some(cause),
                    })?;
                self.delete_reads(ctx, plan, POOL, rows, |row| (row.id, row.status))
                    .await
            }
            Kind::Listener => {
                let rows = self
                    .listeners(ctx, plan.glb)
                    .await
                    .map_err(|cause| CheckFailure {
                        reason: interrupted,
                        cause: Some(cause),
                    })?;
                self.delete_reads(ctx, plan, LISTENER, rows, |row| (row.id, row.status))
                    .await
            }
        }
        .map_err(|cause| CheckFailure {
            reason: interrupted,
            cause: Some(cause),
        })?;
        match reads.decision(plan.id, &self.0.evidence)? {
            DeleteState::Pending => Ok(false),
            DeleteState::Absent => {
                let survivors = self
                    .survivors(ctx, plan)
                    .await
                    .map_err(|cause| CheckFailure {
                        reason: unproven,
                        cause: Some(cause),
                    })?;
                survivors
                    .recheck(&self.0.evidence)
                    .map_err(|cause| CheckFailure {
                        reason: unproven,
                        cause: Some(cause),
                    })?;
                Ok(matches!(
                    reads.decision(plan.id, &self.0.evidence)?,
                    DeleteState::Absent
                ))
            }
        }
    }

    async fn delete_reads<T: WireField>(
        &mut self,
        ctx: &RequestContext,
        plan: &DeletePlan<'_>,
        template: &'static str,
        rows: Guarded<Vec<T>>,
        fields: impl Fn(T) -> (String, String),
    ) -> Result<DeleteReads, Box<Error>> {
        let listed = rows.try_map(&self.0.evidence, |rows| {
            Ok(rows.into_iter().any(|row| fields(row).0 == plan.id))
        })?;
        let detail = self
            .read_guard_reply(ctx, template, &[plan.glb, plan.id], false)
            .await?
            .try_map(&self.0.evidence, |reply| {
                Ok(match reply {
                    GuardReply::Present(row) => Some(fields(row)),
                    GuardReply::Absent(_) => None,
                })
            })?;
        Ok(DeleteReads { listed, detail })
    }

    async fn survivors(
        &mut self,
        ctx: &RequestContext,
        plan: &DeletePlan<'_>,
    ) -> Result<Guarded<()>, Box<Error>> {
        let checkpoint = self.0.evidence.guard_checkpoint();
        let parent = self.parent(ctx, plan.glb).await?;
        match plan.kind {
            Kind::Pool => {
                let reply = self
                    .read_guard_reply::<Vec<Group>>(ctx, GROUPS, &[plan.glb, plan.pool], false)
                    .await?;
                let expected = &plan.group.recheck(&self.0.evidence)?.id;
                if let GuardReply::Present(groups) = reply.recheck(&self.0.evidence)? {
                    self.unique(groups.iter().map(|row| row.id.as_str()), PathIds::Strict)?;
                    if groups.iter().any(|row| row.id == *expected) {
                        return Err(refuse("the member group is still listed"));
                    }
                }
            }
            Kind::Listener => {
                let pool = self.in_pool(ctx, plan.glb, plan.pool).await?;
                let groups = self.groups(ctx, plan.glb, plan.pool).await?;
                pool.recheck(&self.0.evidence)?;
                let expected = &plan.group.recheck(&self.0.evidence)?.id;
                if !groups
                    .recheck(&self.0.evidence)?
                    .iter()
                    .any(|row| row.id == *expected)
                {
                    return Err(refuse("the member group did not survive"));
                }
            }
        }
        parent.recheck(&self.0.evidence)?;
        Guarded::from_checkpoint((), &self.0.evidence, checkpoint)
    }
}

struct CheckFailure {
    reason: &'static str,
    cause: Option<Box<Error>>,
}

impl From<&'static str> for CheckFailure {
    fn from(reason: &'static str) -> Self {
        Self {
            reason,
            cause: None,
        }
    }
}

struct DeleteReads {
    listed: Guarded<bool>,
    detail: Guarded<Option<(String, String)>>,
}

enum DeleteState {
    Pending,
    Absent,
}

impl DeleteReads {
    fn decision(&self, id: &str, evidence: &Evidence) -> Result<DeleteState, &'static str> {
        let unproven = "delete absence could not be proven";
        let listed = *self.listed.recheck(evidence).map_err(|_| unproven)?;
        let detail = self.detail.recheck(evidence).map_err(|_| unproven)?;
        match detail {
            Some((actual, _)) if actual != id => Err("delete detail ID mismatch"),
            Some((_, status)) if !deleting(status) => Err("delete status unrecognized"),
            Some(_) => Ok(DeleteState::Pending),
            None if listed => Ok(DeleteState::Pending),
            None => Ok(DeleteState::Absent),
        }
    }
}

fn stable(pool: &PoolWire) -> bool {
    state(&pool.status) == State::Active
        && pool
            .health
            .as_ref()
            .is_some_and(|h| state(&h.status) == State::Active)
}

fn deleting(status: &str) -> bool {
    healthy(status)
        || matches!(
            status.to_ascii_uppercase().as_str(),
            "ACTIVE" | "DELETING" | "PENDING"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::globalloadbalancer::child::retained_guards::{child, rotating_at};
    use crate::globalloadbalancer::tests::{
        child::{child_fixture, empty, ok, servers_with_auth},
        response,
    };

    #[tokio::test]
    async fn glb_pool_absence_rechecks_the_preflight_group_before_comparison() {
        let fixture = child_fixture();
        for (note, expected) in [("ordinary-note", true), ("toy-final-bearer", false)] {
            let mut group = fixture["group"].clone();
            group["note"] = json!(note);
            let replies = vec![
                ok(&json!([group])),
                ok(&group),
                empty(),
                response("404 Not Found", "{}"),
                ok(&fixture["parent"]),
                empty(),
            ];
            let (client, glb, _) =
                servers_with_auth(replies, vec![], true, rotating_at(6, "toy-final-bearer"));
            let mut child = child(&client);
            let ctx = RequestContext::new().0;
            let group = child.sole_group(&ctx, "glb-1", "pool-1").await.unwrap();
            let plan = DeletePlan {
                kind: Kind::Pool,
                glb: "glb-1",
                id: "pool-1",
                pool: "pool-1",
                group: &group,
            };
            assert_eq!(
                child
                    .check_delete(&ctx, &plan)
                    .await
                    .is_ok_and(|settled| settled),
                expected
            );
            assert_eq!(glb.requests(), 6);
        }
    }

    #[tokio::test]
    async fn glb_final_survivor_read_rechecks_the_original_absence_reply() {
        let fixture = child_fixture();
        for (body, expected) in [
            (json!({"code":"internal_error"}), Ok(true)),
            (
                json!({"code":"internal_error", "note":"toy-final-bearer"}),
                Err("delete absence could not be proven"),
            ),
        ] {
            let replies = vec![
                ok(&json!([fixture["group"]])),
                ok(&fixture["group"]),
                empty(),
                response("500 Internal Server Error", body.to_string()),
                ok(&fixture["parent"]),
                empty(),
            ];
            let (client, glb, _) =
                servers_with_auth(replies, vec![], true, rotating_at(6, "toy-final-bearer"));
            let ctx = RequestContext::new().0;
            let mut child = child(&client);
            let group = child.sole_group(&ctx, "glb-1", "pool-1").await.unwrap();
            let plan = DeletePlan {
                kind: Kind::Pool,
                glb: "glb-1",
                id: "pool-1",
                pool: "pool-1",
                group: &group,
            };
            assert_eq!(
                child
                    .check_delete(&ctx, &plan)
                    .await
                    .map_err(|error| error.reason),
                expected
            );
            assert_eq!(glb.requests(), 6);
        }
    }
}
