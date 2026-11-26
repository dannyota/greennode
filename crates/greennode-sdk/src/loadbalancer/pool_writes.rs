use http::Method;
use serde::{Serialize, Serializer, ser::SerializeStruct};

use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Error, Failure, RequestContext, WriteDispatch};

use super::child_writes::{
    BOUND, ChildTarget, ChildWriter, INTERVAL, WriteResult, encode_body, lb_busy, not_sent,
};
use super::pool_output::PoolProjection;
use super::wire::{CompleteNotFound, DecodeError, Read, invalid_input, screen_input};
use super::{Client, HealthMonitor, Pool, guard};
use guard::shape;

const CREATE: &str = "loadbalancer.CreatePool";
const UPDATE: &str = "loadbalancer.UpdatePool";
const LIST_POOLS: Read = Read {
    id: "loadbalancer.ListPools",
    route: "v2/{}/loadBalancers/{}/pools",
};
const GET_POOL: Read = Read {
    id: "loadbalancer.GetPool",
    route: "v2/{}/loadBalancers/{}/pools/{}",
};
const GET_MONITOR: Read = Read {
    id: "loadbalancer.GetPoolHealthMonitor",
    route: "v2/{}/loadBalancers/{}/pools/{}/healthMonitor",
};

/// Creates a pool and its health monitor.
#[derive(Clone, Debug, Default)]
pub struct CreatePoolInput {
    /// Parent load balancer identifier.
    pub load_balancer_id: String,
    /// Name of the new pool.
    pub name: String,
    /// Protocol used by the pool.
    pub protocol: String,
    /// Protocol used by the health monitor.
    pub health_check_protocol: String,
    /// Load balancing algorithm.
    pub algorithm: String,
    /// Whether session stickiness is enabled.
    pub stickiness: Option<bool>,
    /// Whether backend traffic uses TLS.
    pub tls_encryption: Option<bool>,
    /// Request path for HTTP health checks.
    pub health_check_path: String,
    /// HTTP method for health checks.
    pub health_check_method: String,
    /// HTTP version for health checks.
    pub health_check_http_version: String,
    /// Domain sent with HTTP health checks.
    pub health_check_domain_name: String,
    /// Accepted HTTP health check status.
    pub health_check_success_code: String,
    /// Consecutive successful health checks required.
    pub healthy_threshold: i64,
    /// Consecutive failed health checks required.
    pub unhealthy_threshold: i64,
    /// Seconds between health checks.
    pub health_check_interval: i64,
    /// Health check timeout in seconds.
    pub health_check_timeout: i64,
    /// Returns after the provider accepts the write.
    pub no_wait: bool,
}

/// Holds the pool returned by the create or its confirmation.
#[derive(Clone, Debug, PartialEq)]
pub struct CreatePoolOutput {
    /// The observed or submitted pool.
    pub pool: Pool,
    projection: Option<PoolProjection>,
}

impl Serialize for CreatePoolOutput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut output = serializer.serialize_struct("CreatePoolOutput", 1)?;
        if let Some(projection) = &self.projection {
            output.serialize_field("Pool", projection)?;
        } else {
            output.serialize_field("Pool", &self.pool)?;
        }
        output.end()
    }
}

/// Changes a pool or its health monitor while preserving fields left unset.
#[derive(Clone, Debug, Default, Serialize)]
pub struct UpdatePoolInput {
    /// Parent load balancer identifier.
    #[serde(skip)]
    pub load_balancer_id: String,
    /// Target pool identifier.
    #[serde(skip)]
    pub pool_id: String,
    /// Load balancing algorithm.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub algorithm: Option<String>,
    /// Whether session stickiness is enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stickiness: Option<bool>,
    /// Whether backend traffic uses TLS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_encryption: Option<bool>,
    /// Request path for HTTP health checks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_check_path: Option<String>,
    /// HTTP method for health checks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_check_method: Option<String>,
    /// HTTP version for health checks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_check_http_version: Option<String>,
    /// Domain sent with HTTP health checks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_check_domain_name: Option<String>,
    /// Accepted HTTP health check status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_check_success_code: Option<String>,
    /// Consecutive successful health checks required.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub healthy_threshold: Option<i64>,
    /// Consecutive failed health checks required.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unhealthy_threshold: Option<i64>,
    /// Seconds between health checks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_check_interval: Option<i64>,
    /// Health check timeout in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_check_timeout: Option<i64>,
    /// Returns after the provider accepts the write.
    #[serde(skip)]
    pub no_wait: bool,
}

/// Holds the pool returned by the update or its confirmation.
#[derive(Clone, Debug, PartialEq)]
pub struct UpdatePoolOutput {
    /// The observed or submitted pool.
    pub pool: Pool,
    projection: Option<PoolProjection>,
}

impl Serialize for UpdatePoolOutput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut output = serializer.serialize_struct("UpdatePoolOutput", 1)?;
        if let Some(projection) = &self.projection {
            output.serialize_field("Pool", projection)?;
        } else {
            output.serialize_field("Pool", &self.pool)?;
        }
        output.end()
    }
}

const MONITOR: Shape<'_> = shape!("timeout", "createdAt", "updatedAt", "domainName",
    "httpVersion", "healthCheckProtocol", "interval", "healthyThreshold", "unhealthyThreshold",
    "healthCheckMethod", "healthCheckPath", "successCode", "progressStatus", "displayStatus";);
const MEMBER: Shape<'_> = shape!("uuid", "address", "protocolPort", "weight", "monitorPort",
    "subnetId", "name", "poolId", "typeCreate", "backup", "displayStatus", "createdAt",
    "updateAt", "createdBy", "progressStatus";);
const POOL: Shape<'_> = shape!("uuid", "name", "protocol", "description", "loadBalanceMethod",
    "displayStatus", "progressStatus", "stickiness", "tlsEncryption";
    "members" => Shape::Array(&MEMBER), "healthMonitor" => MONITOR);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PoolBody {
    pool_name: String,
    pool_protocol: String,
    algorithm: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    stickiness: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tls_encryption: Option<bool>,
    health_monitor: MonitorBody,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MonitorBody {
    #[serde(skip_serializing_if = "String::is_empty")]
    health_check_protocol: String,
    healthy_threshold: i64,
    unhealthy_threshold: i64,
    interval: i64,
    timeout: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    health_check_path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    health_check_method: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    http_version: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    domain_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    success_code: String,
}

impl MonitorBody {
    fn resolve_http(&mut self, protocol: &str) {
        match protocol {
            "HTTP" => {
                default_string(&mut self.health_check_path, "/");
                default_string(&mut self.health_check_method, "GET");
                default_string(&mut self.http_version, "1.1");
                default_string(&mut self.success_code, "200");
            }
            "HTTPS" => {}
            _ => {
                self.health_check_path.clear();
                self.health_check_method.clear();
                self.http_version.clear();
                self.domain_name.clear();
                self.success_code.clear();
            }
        }
    }

    fn submitted(self, mut monitor: HealthMonitor) -> HealthMonitor {
        monitor.healthy_threshold = self.healthy_threshold;
        monitor.unhealthy_threshold = self.unhealthy_threshold;
        monitor.interval = self.interval;
        monitor.timeout = self.timeout;
        monitor.health_check_path = nonempty(self.health_check_path);
        monitor.health_check_method = nonempty(self.health_check_method);
        monitor.http_version = nonempty(self.http_version);
        monitor.domain_name = nonempty(self.domain_name);
        monitor.success_code = nonempty(self.success_code);
        monitor
    }
}

impl PoolBody {
    fn submit(self, mut pool: Pool, monitor: HealthMonitor) -> Pool {
        pool.load_balance_method = self.algorithm;
        if let Some(stickiness) = self.stickiness {
            pool.stickiness = stickiness;
        }
        if let Some(tls_encryption) = self.tls_encryption {
            pool.tls_encryption = tls_encryption;
        }
        pool.health_monitor = Some(self.health_monitor.submitted(monitor));
        pool
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Creates a pool and optionally waits for the pool and load balancer to settle.
    pub async fn create_pool(
        &self,
        ctx: &RequestContext,
        input: CreatePoolInput,
    ) -> WriteResult<CreatePoolOutput> {
        let CreatePoolInput {
            load_balancer_id,
            name,
            protocol,
            health_check_protocol,
            mut algorithm,
            stickiness,
            tls_encryption,
            health_check_path,
            health_check_method,
            health_check_http_version,
            health_check_domain_name,
            health_check_success_code,
            healthy_threshold,
            unhealthy_threshold,
            health_check_interval,
            health_check_timeout,
            no_wait,
        } = input;
        let ids = [("LoadBalancerID", load_balancer_id.as_str())];
        let mut write =
            ChildWriter::new(self, ctx, CREATE, &load_balancer_id, &ids).map_err(not_sent)?;
        required(
            CREATE,
            [
                ("Name", name.as_str()),
                ("Protocol", &protocol),
                ("HealthCheckProtocol", &health_check_protocol),
            ],
        )
        .map_err(not_sent)?;
        screen_input(
            &write.evidence,
            [
                name.as_str(),
                &protocol,
                &health_check_protocol,
                &health_check_path,
                &health_check_method,
                &health_check_http_version,
                &health_check_domain_name,
                &health_check_success_code,
            ],
        )
        .map_err(not_sent)?;
        check_http_fields(
            CREATE,
            &health_check_protocol,
            [
                &health_check_path,
                &health_check_method,
                &health_check_http_version,
                &health_check_domain_name,
                &health_check_success_code,
            ],
        )
        .map_err(not_sent)?;
        default_string(&mut algorithm, "ROUND_ROBIN");
        let mut health_monitor = MonitorBody {
            health_check_protocol: health_check_protocol.clone(),
            healthy_threshold: default_int(healthy_threshold, 3),
            unhealthy_threshold: default_int(unhealthy_threshold, 3),
            interval: default_int(health_check_interval, 30),
            timeout: default_int(health_check_timeout, 5),
            health_check_path,
            health_check_method,
            http_version: health_check_http_version,
            domain_name: health_check_domain_name,
            success_code: health_check_success_code,
        };
        health_monitor.resolve_http(&health_check_protocol);
        let body = PoolBody {
            pool_name: name.clone(),
            pool_protocol: protocol.clone(),
            algorithm,
            stickiness: protocol_bool(&protocol, stickiness, false),
            tls_encryption: protocol_bool(&protocol, tls_encryption, false),
            health_monitor,
        };
        let bytes = encode_body(&body, &write.evidence).map_err(not_sent)?;
        let _lock = self
            .lock_load_balancer(ctx, &load_balancer_id)
            .await
            .map_err(|error| not_sent(Box::new(error)))?;
        let ready = write.ready(ctx, None).await.map_err(not_sent)?;
        screen_input(&write.evidence, [protocol.as_str()]).map_err(not_sent)?;
        if ready.lb_type == "Layer 7" && protocol != "HTTP" {
            return Err(not_sent(invalid_input(&format!(
                "{CREATE}: a Layer 7 load balancer accepts only HTTP pools, got {protocol:?}"
            ))));
        }
        let target = ChildTarget {
            read: &LIST_POOLS,
            ids: &ids,
            ..pool_target("", &ids)
        };
        let (status, response) = write
            .send(ctx, Method::POST, &target, bytes, None)
            .await
            .map_err(|failure| write.create_failure(failure, "list-pools"))?;
        let uuid = write.acknowledged_id(status, &response, "list-pools", no_wait)?;
        let fallback = body.submit(
            Pool {
                uuid: uuid.clone(),
                name,
                protocol,
                ..Pool::default()
            },
            HealthMonitor {
                health_check_protocol,
                ..HealthMonitor::default()
            },
        );
        let ids = [
            ("LoadBalancerID", load_balancer_id.as_str()),
            ("PoolID", uuid.as_str()),
        ];
        let target = pool_target(&uuid, &ids);
        finish(
            ctx,
            &mut write,
            &target,
            no_wait,
            true,
            fallback,
            |pool, projection| CreatePoolOutput { pool, projection },
        )
        .await
    }

    /// Updates a pool using its current settings for fields left unset.
    pub async fn update_pool(
        &self,
        ctx: &RequestContext,
        input: UpdatePoolInput,
    ) -> WriteResult<UpdatePoolOutput> {
        let ids = [
            ("LoadBalancerID", input.load_balancer_id.as_str()),
            ("PoolID", input.pool_id.as_str()),
        ];
        let mut write =
            ChildWriter::new(self, ctx, UPDATE, &input.load_balancer_id, &ids).map_err(not_sent)?;
        if encode_body(&input, &write.evidence)
            .map_err(not_sent)?
            .as_ref()
            == b"{}"
        {
            return Err(not_sent(invalid_input(&format!(
                "{UPDATE} requires at least one field to change"
            ))));
        }
        let target = pool_target(&input.pool_id, &ids);
        let _lock = self
            .lock_load_balancer(ctx, &input.load_balancer_id)
            .await
            .map_err(|error| not_sent(Box::new(error)))?;
        let ready = write.ready(ctx, Some(&target)).await.map_err(not_sent)?;
        let pool: Pool = GET_POOL
            .parse(ready.child.as_deref().unwrap_or("null"), &write.evidence)
            .map_err(not_sent)?;
        let row = write
            .read_data(ctx, &GET_MONITOR, &ids)
            .await
            .map_err(not_sent)?;
        let monitor: HealthMonitor =
            guard::parse(&GET_MONITOR, &row, &MONITOR, &write.evidence).map_err(not_sent)?;
        if monitor.health_check_protocol.is_empty() {
            return Err(not_sent(
                GET_MONITOR.decode_error(&write.evidence, DecodeError::InvalidShape),
            ));
        }
        screen_input(
            &write.evidence,
            [pool.protocol.as_str(), &monitor.health_check_protocol],
        )
        .map_err(not_sent)?;
        let body = merged(&input, &pool, &monitor).map_err(not_sent)?;
        let bytes = encode_body(&body, &write.evidence).map_err(not_sent)?;
        write
            .send(ctx, Method::PUT, &target, bytes, Some(&target))
            .await?;
        let fallback = if input.no_wait {
            body.submit(pool, monitor)
        } else {
            pool
        };
        finish(
            ctx,
            &mut write,
            &target,
            input.no_wait,
            false,
            fallback,
            |pool, projection| UpdatePoolOutput { pool, projection },
        )
        .await
    }
}

fn merged(
    input: &UpdatePoolInput,
    pool: &Pool,
    monitor: &HealthMonitor,
) -> Result<PoolBody, Box<Error>> {
    check_http_fields(
        UPDATE,
        &monitor.health_check_protocol,
        [
            or_blank(&input.health_check_path),
            or_blank(&input.health_check_method),
            or_blank(&input.health_check_http_version),
            or_blank(&input.health_check_domain_name),
            or_blank(&input.health_check_success_code),
        ],
    )?;
    let mut health_monitor = MonitorBody {
        health_check_protocol: String::new(),
        healthy_threshold: input.healthy_threshold.unwrap_or(monitor.healthy_threshold),
        unhealthy_threshold: input
            .unhealthy_threshold
            .unwrap_or(monitor.unhealthy_threshold),
        interval: input.health_check_interval.unwrap_or(monitor.interval),
        timeout: input.health_check_timeout.unwrap_or(monitor.timeout),
        health_check_path: merge_string(&input.health_check_path, &monitor.health_check_path),
        health_check_method: merge_string(&input.health_check_method, &monitor.health_check_method),
        http_version: merge_string(&input.health_check_http_version, &monitor.http_version),
        domain_name: merge_string(&input.health_check_domain_name, &monitor.domain_name),
        success_code: merge_string(&input.health_check_success_code, &monitor.success_code),
    };
    health_monitor.resolve_http(&monitor.health_check_protocol);
    Ok(PoolBody {
        pool_name: String::new(),
        pool_protocol: String::new(),
        algorithm: input
            .algorithm
            .clone()
            .unwrap_or_else(|| pool.load_balance_method.clone()),
        stickiness: protocol_bool(&pool.protocol, input.stickiness, pool.stickiness),
        tls_encryption: protocol_bool(&pool.protocol, input.tls_encryption, pool.tls_encryption),
        health_monitor,
    })
}

fn check_http_fields(operation: &str, protocol: &str, values: [&str; 5]) -> Result<(), Box<Error>> {
    let names = [
        "HealthCheckPath",
        "HealthCheckMethod",
        "HealthCheckHTTPVersion",
        "HealthCheckDomainName",
        "HealthCheckSuccessCode",
    ];
    if !matches!(protocol, "HTTP" | "HTTPS")
        && let Some((field, _)) = names
            .into_iter()
            .zip(values)
            .find(|(_, value)| !value.is_empty())
    {
        return Err(invalid_input(&format!(
            "{operation}: {field} is only valid when HealthCheckProtocol is HTTP or HTTPS, got {protocol:?}"
        )));
    }
    Ok(())
}

fn default_string(value: &mut String, fallback: &str) {
    if value.is_empty() {
        value.push_str(fallback);
    }
}

fn default_int(value: i64, fallback: i64) -> i64 {
    if value == 0 { fallback } else { value }
}

fn nonempty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn merge_string(input: &Option<String>, current: &Option<String>) -> String {
    input
        .as_ref()
        .or(current.as_ref())
        .cloned()
        .unwrap_or_default()
}

fn or_blank(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or_default()
}

fn protocol_bool(protocol: &str, input: Option<bool>, current: bool) -> Option<bool> {
    input.or_else(|| (protocol == "HTTP" || current).then_some(current))
}

fn required<'a>(
    operation: &str,
    fields: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<(), Box<Error>> {
    for (name, value) in fields {
        if value.is_empty() {
            return Err(invalid_input(&format!("{operation} requires {name}")));
        }
    }
    Ok(())
}

fn pool_target<'a>(id: &'a str, ids: &'a [(&'static str, &'a str)]) -> ChildTarget<'a> {
    ChildTarget {
        kind: "pool",
        id,
        read: &GET_POOL,
        ids,
        shape: &POOL,
        validate: |read, row, evidence| read.parse::<Pool>(row, evidence).map(|_| ()),
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
async fn finish<O>(
    ctx: &RequestContext,
    write: &mut ChildWriter<'_>,
    target: &ChildTarget<'_>,
    no_wait: bool,
    creating: bool,
    mut pool: Pool,
    output: impl FnOnce(Pool, Option<PoolProjection>) -> O,
) -> WriteResult<O> {
    let result = if no_wait {
        Ok(())
    } else {
        settle(ctx, write, target, creating, &mut pool).await
    };
    let retained = PoolProjection::retain(&mut pool, &write.evidence)
        .map(|projection| output(pool, projection))
        .map_err(|()| GET_POOL.decode_error(&write.evidence, DecodeError::ReflectedCredential));
    match (result, retained) {
        (Ok(()), Ok(output)) => Ok(output),
        (Ok(()), Err(error)) => Err(Failure::new(*error, None, WriteDispatch::MayHaveBeenSent)),
        (Err(error), retained) => {
            let partial = if write.evidence.can_retain_partial() {
                retained.ok()
            } else {
                None
            };
            Err(Failure::new(
                *error,
                partial,
                WriteDispatch::MayHaveBeenSent,
            ))
        }
    }
}

async fn settle(
    ctx: &RequestContext,
    write: &mut ChildWriter<'_>,
    target: &ChildTarget<'_>,
    creating: bool,
    last: &mut Pool,
) -> Result<(), Box<Error>> {
    let clock = write.client.session.clock();
    let wait = ctx.child_with_deadline(clock.monotonic_now() + BOUND);
    loop {
        wait.child_with_deadline(clock.monotonic_now() + INTERVAL)
            .wait(clock)
            .await;
        write.confirm_time(ctx, &wait, target)?;
        let row = write.read_data(&wait, &GET_POOL, target.ids).await;
        write.confirm_time(ctx, &wait, target)?;
        let row = match row {
            Ok(row) => row,
            Err(error) if creating && error.detail::<CompleteNotFound>().is_some() => continue,
            Err(error) => return Err(write.unconfirmed(target, *error)),
        };
        let pool: Pool = guard::parse(&GET_POOL, &row, &POOL, &write.evidence)
            .map_err(|error| write.unconfirmed(target, *error))?;
        if pool.uuid != target.id || pool.progress_status.is_empty() {
            let error = GET_POOL.decode_error(&write.evidence, DecodeError::InvalidShape);
            return Err(write.unconfirmed(target, *error));
        }
        screen_input(&write.evidence, [pool.uuid.as_str(), &pool.progress_status])
            .map_err(|error| write.unconfirmed(target, *error))?;
        *last = pool;
        match last.progress_status.as_str() {
            "ERROR" => return Err(write.failed(target.kind, target.id)),
            "CREATED" => {
                let parent = write.parent(&wait).await;
                write.confirm_time(ctx, &wait, target)?;
                let (status, _) = parent.map_err(|error| write.unconfirmed(target, *error))?;
                if status == "ERROR" {
                    return Err(write.failed("load balancer", write.lb_id));
                }
                if !lb_busy(&status) {
                    return Ok(());
                }
            }
            _ => {}
        }
    }
}
