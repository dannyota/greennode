use serde_json::json;

use super::model::Acceptance;
use super::*;

const CREATE_POOL: &str = "globalloadbalancer.CreatePool";
const CREATE_LISTENER: &str = "globalloadbalancer.CreateListener";
const CLIENT_TIMEOUT: i64 = 50;
const CONNECTION_TIMEOUT: i64 = 5;
const MEMBER_TIMEOUT: i64 = 50;

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Creates one TCP pool and private backend group and confirms the exact configuration.
    pub async fn create_pool(
        &self,
        ctx: &RequestContext,
        input: CreatePoolInput,
    ) -> Result<CreatePoolOutput, Failure<CreatePoolOutput>> {
        let glb = input.load_balancer_id.as_str();
        required(
            CREATE_POOL,
            &[
                ("LoadBalancerID", !glb.is_empty()),
                ("Name", !input.name.is_empty()),
                ("MemberName", !input.member_name.is_empty()),
                ("BackendProjectID", !input.backend_project_id.is_empty()),
                ("BackendServerID", !input.backend_server_id.is_empty()),
                ("BackendVPCID", !input.backend_vpc_id.is_empty()),
                ("BackendSubnetID", !input.backend_subnet_id.is_empty()),
                ("BackendPort", input.backend_port != 0),
            ],
        )
        .map_err(preflight)?;
        ids(
            CREATE_POOL,
            &[
                ("LoadBalancerID", glb),
                ("BackendServerID", &input.backend_server_id),
                ("BackendVPCID", &input.backend_vpc_id),
                ("BackendSubnetID", &input.backend_subnet_id),
                ("BackendProjectID", &input.backend_project_id),
            ],
        )
        .map_err(preflight)?;
        if input.name.trim().is_empty()
            || input.member_name.trim().is_empty()
            || !valid_port(input.backend_port)
        {
            return Err(preflight(refuse(
                "globalloadbalancer.CreatePool requires nonblank names and a port from 1 through 65535",
            )));
        }
        let mut run = self.start_child(CREATE_POOL)?;
        run.screen_input(
            ctx,
            &json!([
                glb,
                input.name,
                input.member_name,
                input.backend_project_id,
                input.backend_server_id,
                input.backend_vpc_id,
                input.backend_subnet_id,
                input.backend_port
            ]),
        )
        .map_err(preflight)?;
        let project = run
            .project(ctx, &input.backend_project_id)
            .await
            .map_err(preflight)?;
        let parent = run.parent(ctx, glb).await.map_err(preflight)?;
        let region = run.region(ctx, &parent.package).await.map_err(preflight)?;
        let address = run
            .placement(
                ctx,
                &project,
                &input.backend_server_id,
                &input.backend_vpc_id,
                &input.backend_subnet_id,
            )
            .await
            .map_err(preflight)?;
        if !run.pools(ctx, glb).await.map_err(preflight)?.is_empty() {
            return Err(preflight(refuse("the load balancer already has a pool")));
        }
        if !run.listeners(ctx, glb).await.map_err(preflight)?.is_empty() {
            return Err(preflight(refuse(
                "the load balancer already has a listener",
            )));
        }
        let body = json!({
            "name": input.name, "protocol": TCP, "algorithm": ALGORITHM,
            "health": {"protocol": TCP, "healthyThreshold": HEALTHY, "unhealthyThreshold": UNHEALTHY, "interval": INTERVAL, "timeout": HEALTH_TIMEOUT},
            "globalPoolMembers": [{"name": input.member_name, "region": region, "type": PRIVATE, "vpcId": input.backend_vpc_id, "trafficDial": TRAFFIC,
                "members": [{"name": input.member_name, "address": address, "subnetId": input.backend_subnet_id, "port": input.backend_port,
                "monitorPort": input.backend_port, "weight": WEIGHT, "backupRole": false}]}]
        });
        let reply = run
            .send(ctx, POOLS, Method::POST, &[glb], Some(&body), |_| None)
            .await?;
        let Some(accepted) = run
            .acceptance(&reply, POOLS)
            .filter(|r| r.glb == glb && PathIds::Strict.allows(&r.id))
        else {
            return Err(run.0.finish(
                Settlement::Unknown("create response lacks a valid pool"),
                None,
            ));
        };
        let partial = |run: &Child<'_>, group: &str| {
            CreatePoolOutput::new(
                super::super::Pool {
                    id: safe_id(&accepted.id, &run.0.evidence),
                    ..Default::default()
                },
                super::super::PoolMember {
                    id: safe_id(group, &run.0.evidence),
                    ..Default::default()
                },
                &run.0.evidence,
            )
        };
        let [group] = accepted.groups.as_slice() else {
            return Err(run.fail(
                "create response lacks a valid member group",
                partial(&run, ""),
            ));
        };
        if !PathIds::Strict.allows(&group.id) || group.members.len() > 1 {
            return Err(run.fail(
                "create response lacks a valid member group",
                partial(&run, ""),
            ));
        }
        let row = group.members.first().map(|row| row.id.as_str());
        if row.is_some_and(|id| !PathIds::WithUnderscore.allows(id)) {
            return Err(run.fail("create response has an invalid backend", partial(&run, "")));
        }
        let wait = run.0.wait_context(ctx, CREATE_WAIT);
        let reason = loop {
            if run.0.interrupted(&wait) {
                break Settlement::Unknown("create confirmation interrupted");
            }
            let confirmed = run.confirm_pool(&wait, &input, &accepted, &group.id).await;
            let Ok((parent_state, chain)) = confirmed else {
                break Settlement::Unknown("create confirmation could not be proven");
            };
            if run.0.interrupted(&wait) {
                break Settlement::Unknown("create confirmation interrupted");
            }
            if !member(&chain.group).is_some_and(|m| {
                row.is_none_or(|id| m.id == id)
                    && m.name == input.member_name
                    && m.subnet == input.backend_subnet_id
                    && m.address == address
                    && m.port == input.backend_port
            }) || chain.group.name != input.member_name
                || chain.group.region != region
                || chain.group.vpc != input.backend_vpc_id
            {
                break Settlement::Unknown("create confirmation could not be proven");
            }
            match chain.state().max(parent_state) {
                State::Active => {
                    let output = CreatePoolOutput::new(
                        chain.pool.into(),
                        chain.group.into(),
                        &run.0.evidence,
                    );
                    if run.0.safe_output(&output) {
                        return Ok(output);
                    }
                    break Settlement::Unknown("output withheld");
                }
                State::Transitioning => run.0.pause(&wait).await,
                State::Failed => break Settlement::Failed,
                State::Unknown => break Settlement::Unknown("create status unrecognized"),
            }
        };
        Err(run.0.finish(reason, Some(partial(&run, &group.id))))
    }

    /// Creates a TCP listener with explicitly allowed IPv4 sources, then confirms read-back.
    pub async fn create_listener(
        &self,
        ctx: &RequestContext,
        input: CreateListenerInput,
    ) -> Result<CreateListenerOutput, Failure<CreateListenerOutput>> {
        let glb = input.load_balancer_id.as_str();
        let pool = input.pool_id.as_str();
        required(
            CREATE_LISTENER,
            &[
                ("LoadBalancerID", !glb.is_empty()),
                ("Name", !input.name.is_empty()),
                ("PoolID", !pool.is_empty()),
                ("Port", input.port != 0),
                ("AllowedCIDRs", !input.allowed_cidrs.is_empty()),
                ("BackendProjectID", !input.backend_project_id.is_empty()),
                ("BackendServerID", !input.backend_server_id.is_empty()),
            ],
        )
        .map_err(preflight)?;
        ids(
            CREATE_LISTENER,
            &[
                ("LoadBalancerID", glb),
                ("PoolID", pool),
                ("BackendServerID", &input.backend_server_id),
                ("BackendProjectID", &input.backend_project_id),
            ],
        )
        .map_err(preflight)?;
        if input.name.trim().is_empty() || !valid_port(input.port) {
            return Err(preflight(refuse(
                "globalloadbalancer.CreateListener requires a nonblank name and a port from 1 through 65535",
            )));
        }
        let cidr = cidr::canonical(CREATE_LISTENER, &input.allowed_cidrs).map_err(preflight)?;
        let mut run = self.start_child(CREATE_LISTENER)?;
        run.screen_input(
            ctx,
            &json!([
                glb,
                pool,
                input.name,
                input.port,
                input.allowed_cidrs,
                input.backend_project_id,
                input.backend_server_id
            ]),
        )
        .map_err(preflight)?;
        let project = run
            .project(ctx, &input.backend_project_id)
            .await
            .map_err(preflight)?;
        let parent = run.parent(ctx, glb).await.map_err(preflight)?;
        let region = run.region(ctx, &parent.package).await.map_err(preflight)?;
        run.no_listeners(ctx, glb).await.map_err(preflight)?;
        let chain = run
            .chain(ctx, glb, pool, None, "")
            .await
            .map_err(preflight)?;
        if chain.state() != State::Active || chain.group.region != region {
            return Err(preflight(refuse(
                "the pool and member group are not settled in the verified region",
            )));
        }
        let before = &chain.group;
        let row = member(before).ok_or_else(|| {
            preflight(refuse(
                "the member group does not match the supported private backend",
            ))
        })?;
        let address = run
            .placement(
                ctx,
                &project,
                &input.backend_server_id,
                &before.vpc,
                &row.subnet,
            )
            .await
            .map_err(preflight)?;
        if address != row.address {
            return Err(preflight(refuse(
                "the pool backend address does not match the server",
            )));
        }
        let body = json!({"name": input.name, "protocol": TCP, "port": input.port,
            "globalPoolId": pool, "allowedCidrs": cidr, "timeoutClient": CLIENT_TIMEOUT,
            "timeoutConnection": CONNECTION_TIMEOUT, "timeoutMember": MEMBER_TIMEOUT});
        let reply = run
            .send(ctx, LISTENERS, Method::POST, &[glb], Some(&body), |_| None)
            .await?;
        let Some(accepted) = run
            .acceptance(&reply, LISTENERS)
            .filter(|row| row.glb == glb && row.pool == pool && PathIds::Strict.allows(&row.id))
        else {
            return Err(run.0.finish(
                Settlement::Unknown("create response lacks a valid listener"),
                None,
            ));
        };
        let wait = run.0.wait_context(ctx, CREATE_WAIT);
        let reason = loop {
            if run.0.interrupted(&wait) {
                break Settlement::Unknown("create confirmation interrupted");
            }
            let Ok(listener) = run
                .confirm_listener(&wait, &input, &accepted.id, before)
                .await
            else {
                break Settlement::Unknown("create confirmation could not be proven");
            };
            if run.0.interrupted(&wait) {
                break Settlement::Unknown("create confirmation interrupted");
            }
            match state(&listener.status) {
                State::Active => {
                    let output = CreateListenerOutput::new(listener.into(), &run.0.evidence);
                    if run.0.safe_output(&output) {
                        return Ok(output);
                    }
                    break Settlement::Unknown("output withheld");
                }
                State::Transitioning => run.0.pause(&wait).await,
                State::Failed => break Settlement::Failed,
                State::Unknown => break Settlement::Unknown("create status unrecognized"),
            }
        };
        let partial = CreateListenerOutput::new(
            super::super::Listener {
                id: safe_id(&accepted.id, &run.0.evidence),
                ..Default::default()
            },
            &run.0.evidence,
        );
        Err(run.0.finish(reason, Some(partial)))
    }
}

impl Child<'_> {
    async fn confirm_pool(
        &mut self,
        ctx: &RequestContext,
        input: &CreatePoolInput,
        accepted: &Acceptance,
        group: &str,
    ) -> Result<(State, Chain), Box<Error>> {
        let glb = input.load_balancer_id.as_str();
        let parent: Parent = self.get(ctx, PARENT, &[glb]).await?;
        let parent_state = state(&parent.status);
        if parent.id != glb
            || parent.kind != "Layer 4"
            || !matches!(parent_state, State::Active | State::Transitioning)
        {
            return Err(refuse(
                "the load balancer is no longer the expected Layer 4 load balancer",
            ));
        }
        let chain = self
            .chain(ctx, glb, &accepted.id, Some(group), &input.name)
            .await?;
        self.no_listeners(ctx, glb).await?;
        Ok((parent_state, chain))
    }

    async fn confirm_listener(
        &mut self,
        ctx: &RequestContext,
        input: &CreateListenerInput,
        id: &str,
        before: &Group,
    ) -> Result<ListenerWire, Box<Error>> {
        let glb = input.load_balancer_id.as_str();
        let pool = input.pool_id.as_str();
        let listener: ListenerWire = self.get(ctx, LISTENER, &[glb, id]).await?;
        let listeners = self.listeners(ctx, glb).await?;
        let chain = self.chain(ctx, glb, pool, Some(&before.id), "").await?;
        if !matches!(listeners.as_slice(), [row] if row.id == id)
            || chain.state() != State::Active
            || !same_backend(&chain.group, before)
            || listener.id != id
            || listener.glb != glb
            || listener.pool != pool
            || listener.name != input.name
            || listener.protocol != TCP
            || listener.port != input.port
            || listener.timeout_client != CLIENT_TIMEOUT
            || listener.timeout_connection != CONNECTION_TIMEOUT
            || listener.timeout_member != MEMBER_TIMEOUT
            || !cidr::same_set(&listener.allowed_cidrs, &input.allowed_cidrs)
        {
            return Err(refuse("create confirmation could not be proven"));
        }
        Ok(listener)
    }
}

fn same_backend(a: &Group, b: &Group) -> bool {
    a.id == b.id
        && a.region == b.region
        && a.vpc == b.vpc
        && member(a).zip(member(b)).is_some_and(|(a, b)| {
            a.id == b.id && a.subnet == b.subnet && a.address == b.address && a.port == b.port
        })
}
