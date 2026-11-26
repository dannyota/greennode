use super::*;
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
        let region = run.region(ctx, &parent.package).await.map_err(preflight)?;
        let listener: ListenerWire = run
            .get(ctx, LISTENER, &[glb, id])
            .await
            .map_err(preflight)?;
        let listeners = run.listeners(ctx, glb).await.map_err(preflight)?;
        if listener.id != id
            || listener.glb != glb
            || listener.protocol != TCP
            || state(&listener.status) != State::Active
            || !matches!(listeners.as_slice(), [row] if row.id == id)
            || !PathIds::Strict.allows(&listener.pool)
        {
            return Err(preflight(refuse(
                "the listener is not an eligible sole TCP listener",
            )));
        }
        let pool = run
            .in_pool(ctx, glb, &listener.pool)
            .await
            .map_err(preflight)?;
        if !stable(&pool) {
            return Err(preflight(refuse("the pool is not in a stable state")));
        }
        let group = run
            .sole_group(ctx, glb, &listener.pool)
            .await
            .map_err(preflight)?;
        if group.region != region {
            return Err(preflight(refuse(
                "the member group is not the expected private group in the configured region",
            )));
        }
        let plan = DeletePlan {
            kind: Kind::Listener,
            glb,
            id,
            pool: &listener.pool,
            group: &group.id,
        };
        run.delete(ctx, plan, DeleteListenerOutput::new).await
    }

    /// Deletes the listener-free pool and its one private group, then verifies their absence.
    pub async fn delete_pool(
        &self,
        ctx: &RequestContext,
        input: DeletePoolInput,
    ) -> Result<DeletePoolOutput, Failure<DeletePoolOutput>> {
        let (glb, id) = (input.load_balancer_id.as_str(), input.pool_id.as_str());
        ids(DELETE_POOL, &[("LoadBalancerID", glb), ("PoolID", id)]).map_err(preflight)?;
        let mut run = self.start_child(DELETE_POOL)?;
        run.screen_input(ctx, &json!([glb, id]))
            .map_err(preflight)?;
        let parent = run.parent(ctx, glb).await.map_err(preflight)?;
        let region = run.region(ctx, &parent.package).await.map_err(preflight)?;
        let pool = run.in_pool(ctx, glb, id).await.map_err(preflight)?;
        if !stable(&pool) {
            return Err(preflight(refuse("the pool is not in a stable state")));
        }
        run.no_listeners(ctx, glb).await.map_err(preflight)?;
        let group = run.sole_group(ctx, glb, id).await.map_err(preflight)?;
        if group.region != region {
            return Err(preflight(refuse(
                "the member group is not the expected private group with one backend",
            )));
        }
        if !healthy(&group.status) || !member(&group).is_some_and(|m| healthy(&m.status)) {
            return Err(preflight(refuse(
                "the backend row is not settled; wait for a health status",
            )));
        }
        let plan = DeletePlan {
            kind: Kind::Pool,
            glb,
            id,
            pool: id,
            group: &group.id,
        };
        run.delete(ctx, plan, DeletePoolOutput::new).await
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
    group: &'a str,
}

impl Child<'_> {
    async fn in_pool(
        &mut self,
        ctx: &RequestContext,
        glb: &str,
        id: &str,
    ) -> Result<PoolWire, Box<Error>> {
        let pools = self.pools(ctx, glb).await?;
        match pools.as_slice() {
            [] => Err(Box::new(local(
                ErrorClass::NotFound,
                "NotFound",
                "vngcloud: resource not found: the load balancer holds no pool",
            ))),
            [pool] if pool.id == id => self.exact_pool(ctx, glb, id, "").await,
            _ => Err(refuse(
                "the load balancer must hold exactly the expected pool",
            )),
        }
    }

    async fn sole_group(
        &mut self,
        ctx: &RequestContext,
        glb: &str,
        pool: &str,
    ) -> Result<Group, Box<Error>> {
        let rows = self.groups(ctx, glb, pool).await?;
        let [row] = rows.as_slice() else {
            return Err(refuse("the pool must hold exactly one member group"));
        };
        self.exact_group(ctx, glb, pool, &row.id).await
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
        .await?;
        let wait = self.0.wait_context(ctx, DELETE_WAIT);
        let reason = loop {
            if self.0.interrupted(&wait) {
                break "delete confirmation interrupted";
            }
            match self.check_delete(&wait, &plan).await {
                Ok(false) => self.0.pause(&wait).await,
                Ok(true) => {
                    if self.0.interrupted(&wait) {
                        break "delete confirmation interrupted";
                    }
                    let out = result(self, true);
                    if self.0.safe_output(&out) {
                        return Ok(out);
                    }
                    break "output withheld";
                }
                Err(reason) => break reason,
            }
        };
        Err(self.fail(reason, result(self, false)))
    }

    async fn check_delete(
        &mut self,
        ctx: &RequestContext,
        plan: &DeletePlan<'_>,
    ) -> Result<bool, &'static str> {
        let interrupted = "delete confirmation interrupted";
        let (present, detail) = match plan.kind {
            Kind::Pool => {
                let rows = self.pools(ctx, plan.glb).await.map_err(|_| interrupted)?;
                let detail: Result<PoolWire, _> = self.get(ctx, POOL, &[plan.glb, plan.id]).await;
                (
                    rows.iter().any(|row| row.id == plan.id),
                    detail.map(|row| (row.id, row.status)),
                )
            }
            Kind::Listener => {
                let rows = self
                    .listeners(ctx, plan.glb)
                    .await
                    .map_err(|_| interrupted)?;
                let detail: Result<ListenerWire, _> =
                    self.get(ctx, LISTENER, &[plan.glb, plan.id]).await;
                (
                    rows.iter().any(|row| row.id == plan.id),
                    detail.map(|row| (row.id, row.status)),
                )
            }
        };
        match detail {
            Ok((id, _)) if id != plan.id => Err("delete detail ID mismatch"),
            Ok((_, status)) if !deleting(&status) => Err("delete status unrecognized"),
            Ok(_) => Ok(false),
            Err(error) if error.detail::<Absent>().is_none() => Err(interrupted),
            Err(_) if present => Ok(false),
            Err(_) => self
                .survivors(ctx, plan)
                .await
                .map(|()| true)
                .map_err(|_| "delete absence could not be proven"),
        }
    }

    async fn survivors(
        &mut self,
        ctx: &RequestContext,
        plan: &DeletePlan<'_>,
    ) -> Result<(), Box<Error>> {
        self.parent(ctx, plan.glb).await?;
        match plan.kind {
            Kind::Pool => match self.groups(ctx, plan.glb, plan.pool).await {
                Ok(groups) if groups.iter().any(|row| row.id == plan.group) => {
                    Err(refuse("the member group is still listed"))
                }
                Ok(_) => Ok(()),
                Err(error) if error.detail::<Absent>().is_some() => Ok(()),
                Err(error) => Err(error),
            },
            Kind::Listener => {
                self.in_pool(ctx, plan.glb, plan.pool).await?;
                let groups = self.groups(ctx, plan.glb, plan.pool).await?;
                if groups.iter().any(|row| row.id == plan.group) {
                    Ok(())
                } else {
                    Err(refuse("the member group did not survive"))
                }
            }
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
