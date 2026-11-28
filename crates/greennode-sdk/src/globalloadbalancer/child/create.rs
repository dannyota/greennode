use crate::wire::product::guard::guarded_poll;
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
        crate::wire::product::exit::finish!(async {
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
            let region = run.region(ctx, &parent).await.map_err(preflight)?;
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
            let pools = run.pools(ctx, glb).await.map_err(preflight)?;
            if !pools
                .recheck(&run.0.evidence)
                .map_err(preflight)?
                .is_empty()
            {
                return Err(preflight(refuse("the load balancer already has a pool")));
            }
            if !run
                .listeners(ctx, glb)
                .await
                .map_err(preflight)?
                .recheck(&run.0.evidence)
                .map_err(preflight)?
                .is_empty()
            {
                return Err(preflight(refuse(
                    "the load balancer already has a listener",
                )));
            }
            let evidence = &run.0.evidence;
            pools.recheck(evidence).map_err(preflight)?;
            let verified_region = region.recheck(evidence).map_err(preflight)?;
            let verified_address = address.recheck(evidence).map_err(preflight)?;
            let body = json!({
                "name": input.name, "protocol": TCP, "algorithm": ALGORITHM,
                "health": {"protocol": TCP, "healthyThreshold": HEALTHY, "unhealthyThreshold": UNHEALTHY, "interval": INTERVAL, "timeout": HEALTH_TIMEOUT},
                "globalPoolMembers": [{"name": input.member_name, "region": verified_region, "type": PRIVATE, "vpcId": input.backend_vpc_id, "trafficDial": TRAFFIC,
                    "members": [{"name": input.member_name, "address": verified_address, "subnetId": input.backend_subnet_id, "port": input.backend_port,
                    "monitorPort": input.backend_port, "weight": WEIGHT, "backupRole": false}]}]
            });
            let reply = run
                .send(ctx, POOLS, Method::POST, &[glb], Some(&body), |_| None)
                .await?;
            let Some(acceptance) = run.acceptance(&reply, POOLS, |row| {
                row.glb == glb && PathIds::Strict.allows(&row.id)
            }) else {
                return Err(run.0.finish(
                    Settlement::Unknown("create response lacks a valid pool"),
                    None,
                ));
            };
            let accepted = acceptance.recheck(&run.0.evidence).map_err(|_| {
                run.0.finish(
                    Settlement::Unknown("create response lacks a valid pool"),
                    None,
                )
            })?;
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
            let group_id = acceptance
                .acknowledged_id(|row| row.groups.first().map(|group| group.id.as_str()))
                .ok_or_else(|| {
                    run.fail(
                        "create response lacks a valid member group",
                        partial(&run, ""),
                    )
                })?;
            let member_id = acceptance.acknowledged_id(|row| {
                row.groups
                    .first()?
                    .members
                    .first()
                    .map(|member| member.id.as_str())
            });
            if member_id.as_ref().is_some_and(|id| {
                !id.recheck(&run.0.evidence)
                    .is_ok_and(|id| PathIds::Strict.allows(id))
            }) {
                return Err(run.fail("create response has an invalid backend", partial(&run, "")));
            }
            let wait = run.0.wait_context(ctx, CREATE_WAIT);
            let reason = guarded_poll!(run.0.evidence, {
                if run.0.interrupted(&wait) {
                    break Settlement::Unknown("create confirmation interrupted");
                }
                let confirmed = run
                    .confirm_pool(&wait, &input, &acceptance, &group_id)
                    .await;
                let Ok((parent_state, chain)) = confirmed else {
                    break Settlement::Unknown("create confirmation could not be proven");
                };
                if run.0.interrupted(&wait) {
                    break Settlement::Unknown("create confirmation interrupted");
                }
                let evidence = &run.0.evidence;
                let (Ok(region), Ok(address)) =
                    (region.recheck(evidence), address.recheck(evidence))
                else {
                    break Settlement::Unknown("create confirmation could not be proven");
                };
                let Ok(row) = member_id
                    .as_ref()
                    .map(|id| id.recheck(evidence))
                    .transpose()
                else {
                    break Settlement::Unknown("create confirmation could not be proven");
                };
                if !member(&chain.group).is_some_and(|m| {
                    row.is_none_or(|id| m.id == id)
                        && m.name == input.member_name
                        && m.subnet == input.backend_subnet_id
                        && m.address == *address
                        && m.port == input.backend_port
                }) || chain.group.name != input.member_name
                    || chain.group.region != *region
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
            });
            let partial = acceptance
                .recheck(&run.0.evidence)
                .ok()
                .and_then(|_| group_id.recheck(&run.0.evidence).ok())
                .map(|id| partial(&run, id));
            Err(run.0.finish(reason, partial))
        })
    }

    /// Creates a TCP listener with explicitly allowed IPv4 sources, then confirms read-back.
    pub async fn create_listener(
        &self,
        ctx: &RequestContext,
        input: CreateListenerInput,
    ) -> Result<CreateListenerOutput, Failure<CreateListenerOutput>> {
        crate::wire::product::exit::finish!(async {
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
            let region = run.region(ctx, &parent).await.map_err(preflight)?;
            run.no_listeners(ctx, glb).await.map_err(preflight)?;
            let chain = run
                .chain(ctx, glb, pool, None, "")
                .await
                .map_err(preflight)?;
            let verified = chain.recheck(&run.0.evidence).map_err(preflight)?;
            if verified.state() != State::Active
                || verified.group.region != *region.recheck(&run.0.evidence).map_err(preflight)?
            {
                return Err(preflight(refuse(
                    "the pool and member group are not settled in the verified region",
                )));
            }
            let before = &verified.group;
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
            let evidence = &run.0.evidence;
            region.recheck(evidence).map_err(preflight)?;
            chain.recheck(evidence).map_err(preflight)?;
            if *address.recheck(evidence).map_err(preflight)? != row.address {
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
            let Some(acceptance) = run.acceptance(&reply, LISTENERS, |row| {
                row.glb == glb && row.pool == pool && PathIds::Strict.allows(&row.id)
            }) else {
                return Err(run.0.finish(
                    Settlement::Unknown("create response lacks a valid listener"),
                    None,
                ));
            };
            let wait = run.0.wait_context(ctx, CREATE_WAIT);
            let reason = guarded_poll!(run.0.evidence, {
                if run.0.interrupted(&wait) {
                    break Settlement::Unknown("create confirmation interrupted");
                }
                let Ok(listener) = run
                    .confirm_listener(&wait, &input, &acceptance, &chain)
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
            });
            let partial = acceptance.recheck(&run.0.evidence).ok().map(|accepted| {
                CreateListenerOutput::new(
                    super::super::Listener {
                        id: safe_id(&accepted.id, &run.0.evidence),
                        ..Default::default()
                    },
                    &run.0.evidence,
                )
            });
            Err(run.0.finish(reason, partial))
        })
    }
}

impl Child<'_> {
    async fn confirm_pool(
        &mut self,
        ctx: &RequestContext,
        input: &CreatePoolInput,
        acceptance: &Guarded<Acceptance>,
        group: &crate::wire::product::guard::AcknowledgedId,
    ) -> Result<(State, Chain), Box<Error>> {
        let glb = input.load_balancer_id.as_str();
        let parent = self
            .read_guard::<Parent>(ctx, PARENT, &[glb], false)
            .await?;
        confirm_parent(glb, parent.recheck(&self.0.evidence)?)?;
        let accepted = acceptance.recheck(&self.0.evidence)?;
        let group_id = group.recheck(&self.0.evidence)?;
        let chain = self
            .chain(ctx, glb, &accepted.id, Some(group_id), &input.name)
            .await?;
        let listeners = self.read_guard(ctx, LISTENERS, &[glb], false).await?;
        acceptance.recheck(&self.0.evidence)?;
        group.recheck(&self.0.evidence)?;
        self.pool_decision(glb, parent, chain, listeners)
    }

    fn pool_decision(
        &self,
        glb: &str,
        parent: Guarded<Parent>,
        chain: Guarded<Chain>,
        listeners: Guarded<Vec<ListenerWire>>,
    ) -> Result<(State, Chain), Box<Error>> {
        let parent_state = confirm_parent(glb, parent.recheck(&self.0.evidence)?)?;
        if !listeners.recheck(&self.0.evidence)?.is_empty() {
            return Err(refuse("the load balancer has a listener"));
        }
        Ok((parent_state, chain.into_checked(&self.0.evidence)?))
    }

    async fn confirm_listener(
        &mut self,
        ctx: &RequestContext,
        input: &CreateListenerInput,
        acceptance: &Guarded<Acceptance>,
        before: &Guarded<Chain>,
    ) -> Result<ListenerWire, Box<Error>> {
        let glb = input.load_balancer_id.as_str();
        let pool = input.pool_id.as_str();
        let listener_id = acceptance
            .acknowledged_id(|row| Some(&row.id))
            .ok_or_else(|| refuse("create response lacks a listener ID"))?;
        let id = listener_id.recheck(&self.0.evidence)?;
        let listener = self.read_guard(ctx, LISTENER, &[glb, id], false).await?;
        acceptance.recheck(&self.0.evidence)?;
        let listeners = self.read_guard(ctx, LISTENERS, &[glb], false).await?;
        let group = &before.recheck(&self.0.evidence)?.group.id;
        let chain = self.chain(ctx, glb, pool, Some(group), "").await?;
        acceptance.recheck(&self.0.evidence)?;
        self.listener_decision(input, id, before, listener, listeners, chain)
    }

    fn listener_decision(
        &self,
        input: &CreateListenerInput,
        id: &str,
        before: &Guarded<Chain>,
        listener: Guarded<ListenerWire>,
        listeners: Guarded<Vec<ListenerWire>>,
        chain: Guarded<Chain>,
    ) -> Result<ListenerWire, Box<Error>> {
        let glb = &input.load_balancer_id;
        let pool = &input.pool_id;
        let before = &before.recheck(&self.0.evidence)?.group;
        let listener = listener.into_checked(&self.0.evidence)?;
        let listeners = listeners.recheck(&self.0.evidence)?;
        let chain = chain.recheck(&self.0.evidence)?;
        if !matches!(listeners.as_slice(), [row] if row.id == id)
            || chain.state() != State::Active
            || !same_backend(&chain.group, before)
            || listener.id != id
            || listener.glb != *glb
            || listener.pool != *pool
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

fn confirm_parent(glb: &str, parent: &Parent) -> Result<State, Box<Error>> {
    let parent_state = state(&parent.status);
    if parent.id != glb
        || parent.kind != "Layer 4"
        || !matches!(parent_state, State::Active | State::Transitioning)
    {
        return Err(refuse(
            "the load balancer is no longer the expected Layer 4 load balancer",
        ));
    }
    Ok(parent_state)
}

fn same_backend(a: &Group, b: &Group) -> bool {
    a.id == b.id
        && a.region == b.region
        && a.vpc == b.vpc
        && member(a).zip(member(b)).is_some_and(|(a, b)| {
            a.id == b.id && a.subnet == b.subnet && a.address == b.address && a.port == b.port
        })
}
