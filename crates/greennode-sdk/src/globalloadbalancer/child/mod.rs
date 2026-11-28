mod alias;
mod catalog;
mod cidr;
mod create;
mod delete;
mod fold_table;
mod model;
mod types;

#[cfg(test)]
#[path = "../tests/child/retained_guards.rs"]
mod retained_guards;

pub use types::*;

use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

use super::load_balancer_writes::decode_with;
use super::wire::{DecodeError, WireField};
use super::write::{
    PROVENANCE, Settlement, Write, guard_operation, local, not_sent, rejected_response,
    write_operation,
};
use super::{Client, invalid_input};
use crate::endpoint::Base;
use crate::quote::{catalog::unique, request};
use crate::wire::product::{
    evidence::Evidence,
    guard::Guarded,
    operation::{Operation, RefusedGuard},
    options::PathIds,
    sender::Response,
};
use crate::wire::strict_json::{self, Shape, ShapeField};
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};
use model::{Group, ListenerWire, Member, Parent, PoolWire};

const POOLS: &str = "v1/global-load-balancers/{}/global-pools";
const POOL: &str = "v1/global-load-balancers/{}/global-pools/{}";
const LISTENERS: &str = "v1/global-load-balancers/{}/global-listeners";
const LISTENER: &str = "v1/global-load-balancers/{}/global-listeners/{}";
const GROUPS: &str = "v1/global-load-balancers/{}/global-pools/{}/pool-members";
const GROUP: &str = "v1/global-load-balancers/{}/global-pools/{}/pool-members/{}";
const PACKAGES: &str = "v1/packages";
const PARENT: &str = "v1/global-load-balancers/{}";
const CREATE_WAIT: Duration = Duration::from_secs(900);
const DELETE_WAIT: Duration = Duration::from_secs(600);
const PROVIDER_TEXT: &str = "request rejected by the API";
const TCP: &str = "TCP";
const ALGORITHM: &str = "ROUND_ROBIN";
const PRIVATE: &str = "PRIVATE";
const HEALTHY: i64 = 3;
const UNHEALTHY: i64 = 3;
const INTERVAL: i64 = 30;
const HEALTH_TIMEOUT: i64 = 5;
const TRAFFIC: i64 = 100;
const WEIGHT: i64 = 1;

struct Child<'a>(Write<'a>);

impl Client {
    #[allow(clippy::result_large_err)]
    fn start_child<O>(&self, id: &'static str) -> Result<Child<'_>, Failure<O>> {
        let write = self.start_write(id)?;
        let vserver = request::Product::Regional
            .endpoint(&self.session)
            .map(Base::configured_url);
        let trusted = vserver
            == self
                .session
                .default_endpoints()
                .vserver
                .as_ref()
                .map(Base::configured_url);
        #[cfg(any(test, feature = "test-support"))]
        let trusted = trusted
            || self
                .session
                .is_write_fixture(crate::testing::Product::GlobalLoadBalancer);
        if trusted {
            Ok(Child(write))
        } else {
            Err(not_sent(local(
                ErrorClass::InvalidConfig,
                "InvalidConfig",
                PROVENANCE,
            )))
        }
    }
}

impl Child<'_> {
    fn screen_input(&mut self, ctx: &RequestContext, values: &Value) -> Result<(), Box<Error>> {
        let session = &self.0.client.session;
        let mut jitter = || session.retry_jitter();
        if let Some(credentials) = session.runtime(ctx, &mut jitter)?.configured_credentials {
            self.0.evidence.absorb(credentials);
        }
        if self.0.evidence.output_reflects(values) {
            Err(refuse("a request value contains a credential"))
        } else {
            Ok(())
        }
    }

    async fn read_guard<T: WireField>(
        &mut self,
        ctx: &RequestContext,
        template: &'static str,
        path: &[&str],
        regional: bool,
    ) -> Result<Guarded<T>, Box<Error>> {
        self.read_guard_reply(ctx, template, path, regional)
            .await?
            .try_map(&self.0.evidence, |reply| match reply {
                GuardReply::Present(value) => Ok(value),
                GuardReply::Absent(error) => Err(error),
            })
    }

    async fn read_guard_reply<T: WireField>(
        &mut self,
        ctx: &RequestContext,
        template: &'static str,
        path: &[&str],
        regional: bool,
    ) -> Result<Guarded<GuardReply<T>>, Box<Error>> {
        let checkpoint = self.0.evidence.guard_checkpoint();
        let op = guard_operation(self.0.id, template);
        let session = &self.0.client.session;
        let base = if regional {
            request::Product::Regional.endpoint(session)
        } else {
            session.endpoints().global_load_balancer.as_ref()
        };
        let reply = self
            .0
            .guard_on(ctx, &op, base, path)
            .await
            .map_err(|failure| {
                let error = failure.into_parts().0;
                if error
                    .detail::<crate::wire::product::operation::MalformedGuard>()
                    .is_some()
                {
                    return Box::new(error.with_outer_detail(DecodeError::MalformedJson));
                }
                if error.detail::<RefusedGuard>().is_some() {
                    return Box::new(error);
                }
                if error.api_error().is_some_and(|api| {
                    api.is_for_operation(self.0.id) && api.observed_status().is_none()
                }) && !error.has_identity(ErrorClass::InvalidInput)
                {
                    self.invalid(DecodeError::InvalidShape)
                } else {
                    Box::new(error)
                }
            })?;
        if !reply.status.is_client_error()
            && !reply.status.is_server_error()
            && reply.status != StatusCode::OK
        {
            return Err(self.invalid(DecodeError::InvalidShape));
        }
        if reply.status != StatusCode::OK {
            let error = Box::new(
                rejected_response(&op, &reply, None, Some(PROVIDER_TEXT))
                    .with_clean_absence(reply.clean_absence()),
            );
            if crate::confirms_absence(&error, op.id, "GET", op.template)
                || (reply.reflection.clean().is_some() && internal_error(&reply))
            {
                return Guarded::from_checkpoint(
                    GuardReply::Absent(error),
                    &self.0.evidence,
                    checkpoint,
                );
            }
            return Err(error);
        }
        let receipt = reply
            .clean_receipt()
            .ok_or_else(|| self.invalid(DecodeError::ReflectedCredential))?;
        if receipt.body().is_empty() || super::wire::go_json(receipt.body()).trim() == "null" {
            return Err(self.invalid(DecodeError::InvalidShape));
        }
        let result: T = decode_canonical(&op, receipt.body(), &T::SHAPE).map_err(|error| {
            self.invalid(
                error
                    .detail::<DecodeError>()
                    .copied()
                    .unwrap_or(DecodeError::InvalidShape),
            )
        })?;
        if !reply.reflection.is_clean() {
            return Err(self.invalid(DecodeError::ReflectedCredential));
        }
        Guarded::from_checkpoint(GuardReply::Present(result), &self.0.evidence, checkpoint)
    }

    fn invalid(&self, detail: DecodeError) -> Box<Error> {
        Box::new(
            request::api(
                self.0.id,
                (!self.0.evidence.text_reflects("200")).then_some(200),
                "InvalidResponse",
                "response failed validation; body withheld",
                false,
            )
            .with_detail(detail),
        )
    }

    #[allow(clippy::result_large_err)]
    async fn send<O: Serialize>(
        &mut self,
        ctx: &RequestContext,
        template: &'static str,
        method: Method,
        path: &[&str],
        body: Option<&Value>,
        partial: impl FnOnce(&Self) -> Option<O>,
    ) -> Result<Response, Failure<O>> {
        let op = write_operation(self.0.id, template, method);
        match self.0.request(ctx, &op, path, body).await {
            Ok(reply) if op.success.contains(&reply.status) => Ok(reply),
            Ok(reply) if reply.status.is_client_error() => Err(Failure::new(
                rejected_response(&op, &reply, None, Some(PROVIDER_TEXT)),
                None,
                WriteDispatch::MayHaveBeenSent,
            )),
            Ok(_) => Err(self
                .0
                .finish(Settlement::Unknown("request unconfirmed"), partial(self))),
            Err(failure) => Err(self.0.rejection_with_message(
                failure,
                partial(self),
                "request unconfirmed",
                Some(PROVIDER_TEXT),
            )),
        }
    }

    fn acceptance(
        &mut self,
        reply: &Response,
        template: &'static str,
        valid: impl FnOnce(&model::Acceptance) -> bool,
    ) -> Option<Guarded<model::Acceptance>> {
        let receipt = reply.clean_receipt()?;
        let op = guard_operation(self.0.id, template);
        let value = decode_canonical(&op, receipt.body(), &model::Acceptance::SHAPE).ok()?;
        if !valid(&value) {
            return None;
        }
        let guarded = receipt.guard(value);
        guarded
            .register_acknowledgement(&mut self.0.evidence, |value| Some(&value.id))
            .ok()?;
        Some(guarded)
    }

    fn fail<O: Serialize>(&self, reason: &'static str, partial: O) -> Failure<O> {
        self.0.finish(Settlement::Unknown(reason), Some(partial))
    }

    async fn parent(
        &mut self,
        ctx: &RequestContext,
        glb: &str,
    ) -> Result<Guarded<Parent>, Box<Error>> {
        let guarded = self
            .read_guard::<Parent>(ctx, PARENT, &[glb], false)
            .await?;
        let parent = guarded.recheck(&self.0.evidence)?;
        if parent.id != glb
            || parent.kind != "Layer 4"
            || state(&parent.status) != State::Active
            || parent.package.is_empty()
        {
            return Err(refuse(
                "the load balancer is not an eligible Layer 4 load balancer",
            ));
        }
        Ok(guarded)
    }

    async fn pools(
        &mut self,
        ctx: &RequestContext,
        glb: &str,
    ) -> Result<Guarded<Vec<PoolWire>>, Box<Error>> {
        self.list(ctx, POOLS, &[glb], |row: &PoolWire| &row.id)
            .await
    }

    async fn listeners(
        &mut self,
        ctx: &RequestContext,
        glb: &str,
    ) -> Result<Guarded<Vec<ListenerWire>>, Box<Error>> {
        self.list(ctx, LISTENERS, &[glb], |row: &ListenerWire| &row.id)
            .await
    }

    async fn groups(
        &mut self,
        ctx: &RequestContext,
        glb: &str,
        pool: &str,
    ) -> Result<Guarded<Vec<Group>>, Box<Error>> {
        self.list(ctx, GROUPS, &[glb, pool], |row: &Group| &row.id)
            .await
    }

    async fn list<T: WireField>(
        &mut self,
        ctx: &RequestContext,
        template: &'static str,
        path: &[&str],
        id: impl Fn(&T) -> &str,
    ) -> Result<Guarded<Vec<T>>, Box<Error>> {
        let rows = self
            .read_guard::<Vec<T>>(ctx, template, path, false)
            .await?;
        self.unique(
            rows.recheck(&self.0.evidence)?.iter().map(id),
            PathIds::Strict,
        )?;
        Ok(rows)
    }

    fn unique<'a>(
        &self,
        ids: impl IntoIterator<Item = &'a str>,
        rule: PathIds,
    ) -> Result<(), Box<Error>> {
        unique(self.0.id, ids, rule).map_err(|_| self.invalid(DecodeError::InvalidShape))
    }

    async fn no_listeners(&mut self, ctx: &RequestContext, glb: &str) -> Result<(), Box<Error>> {
        if self
            .listeners(ctx, glb)
            .await?
            .recheck(&self.0.evidence)?
            .is_empty()
        {
            Ok(())
        } else {
            Err(refuse("the load balancer has a listener"))
        }
    }

    async fn exact_pool(
        &mut self,
        ctx: &RequestContext,
        glb: &str,
        id: &str,
        name: &str,
    ) -> Result<Guarded<PoolWire>, Box<Error>> {
        let guarded = self
            .read_guard::<PoolWire>(ctx, POOL, &[glb, id], false)
            .await?;
        let pool = guarded.recheck(&self.0.evidence)?;
        if pool.id != id
            || pool.glb != glb
            || (!name.is_empty() && pool.name != name)
            || pool.protocol != TCP
            || pool.algorithm != ALGORITHM
            || pool.health.as_ref().is_none_or(|h| {
                h.pool != id
                    || h.glb != glb
                    || h.protocol != TCP
                    || h.healthy_threshold != HEALTHY
                    || h.unhealthy_threshold != UNHEALTHY
                    || h.interval != INTERVAL
                    || h.timeout != HEALTH_TIMEOUT
            })
        {
            return Err(refuse(
                "the pool does not match the supported TCP configuration",
            ));
        }
        Ok(guarded)
    }

    async fn exact_group(
        &mut self,
        ctx: &RequestContext,
        glb: &str,
        pool: &str,
        group: &str,
    ) -> Result<Guarded<Group>, Box<Error>> {
        if !PathIds::Strict.allows(group) {
            return Err(self.invalid(DecodeError::InvalidShape));
        }
        let guarded = self
            .read_guard::<Group>(ctx, GROUP, &[glb, pool, group], false)
            .await?;
        let row = guarded.recheck(&self.0.evidence)?;
        if row.id != group
            || row.pool != pool
            || row.glb != glb
            || row.kind != PRIVATE
            || row.traffic_dial != TRAFFIC
            || member(row).is_none_or(|m| {
                !PathIds::WithUnderscore.allows(&m.id)
                    || m.group != group
                    || m.glb != glb
                    || m.backup_role != Some(false)
                    || m.weight != WEIGHT
                    || !valid_port(m.port)
                    || m.monitor_port != m.port
                    || m.subnet.is_empty()
                    || m.address.is_empty()
            })
        {
            return Err(refuse(
                "the member group does not match the supported private backend",
            ));
        }
        Ok(guarded)
    }

    async fn chain(
        &mut self,
        ctx: &RequestContext,
        glb: &str,
        pool: &str,
        group: Option<&str>,
        name: &str,
    ) -> Result<Guarded<Chain>, Box<Error>> {
        let checkpoint = self.0.evidence.guard_checkpoint();
        let pools = self.pools(ctx, glb).await?;
        if !matches!(pools.recheck(&self.0.evidence)?.as_slice(), [row] if row.id == pool) {
            return Err(refuse(
                "the load balancer must hold exactly the expected pool",
            ));
        }
        let pool_detail = self.exact_pool(ctx, glb, pool, name).await?;
        let rows = self.groups(ctx, glb, pool).await?;
        let [row] = rows.recheck(&self.0.evidence)?.as_slice() else {
            return Err(refuse(
                "the pool must hold exactly the expected member group",
            ));
        };
        if group.is_some_and(|id| row.id != id) {
            return Err(refuse(
                "the pool must hold exactly the expected member group",
            ));
        }
        let group = self.exact_group(ctx, glb, pool, &row.id).await?;
        let evidence = &self.0.evidence;
        pools.recheck(evidence)?;
        rows.recheck(evidence)?;
        let chain = Chain {
            pool: pool_detail.into_checked(evidence)?,
            group: group.into_checked(evidence)?,
        };
        Guarded::from_checkpoint(chain, evidence, checkpoint)
    }
}

enum GuardReply<T> {
    Present(T),
    Absent(Box<Error>),
}

fn decode_canonical<T: serde::de::DeserializeOwned + Default>(
    op: &Operation,
    body: &[u8],
    shape: &Shape<'_>,
) -> Result<T, Box<Error>> {
    decode_with(op, body, shape, strict_json::validate_canonical_shape)
}

fn internal_error(reply: &Response) -> bool {
    if reply.status != StatusCode::INTERNAL_SERVER_ERROR || !reply.reflection.is_clean() {
        return false;
    }
    let op = guard_operation("", "");
    let shape = Shape::Object(&[ShapeField {
        name: "code",
        shape: Shape::Any,
    }]);
    #[derive(Default, serde::Deserialize)]
    struct Code {
        code: Option<String>,
    }
    reply.clean_receipt().is_some_and(|receipt| {
        decode_canonical::<Code>(&op, receipt.body(), &shape)
            .is_ok_and(|envelope| envelope.code.as_deref() == Some("internal_error"))
    })
}

#[expect(
    clippy::boxed_local,
    reason = "Converts internal boxed errors at the public Failure boundary"
)]
fn preflight<O>(error: Box<Error>) -> Failure<O> {
    not_sent(*error)
}

fn refuse(message: &str) -> Box<Error> {
    Box::new(invalid_input(message.into()))
}

fn valid_port(port: i64) -> bool {
    (1..=65_535).contains(&port)
}

fn ids(op: &'static str, fields: &[(&str, &str)]) -> Result<(), Box<Error>> {
    let operation = guard_operation(op, "");
    super::path_ids(&operation, fields)
        .map(|_| ())
        .map_err(|message| refuse(&message))
}

fn required(op: &'static str, fields: &[(&str, bool)]) -> Result<(), Box<Error>> {
    if let Some((field, _)) = fields.iter().find(|(_, present)| !present) {
        Err(refuse(&format!("{op} requires {field}")))
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum State {
    Active,
    Transitioning,
    Failed,
    Unknown,
}

fn state(status: &str) -> State {
    match status.to_ascii_uppercase().as_str() {
        "ACTIVE" => State::Active,
        "CREATING" | "PENDING" | "PROVISIONING" | "UPDATING" => State::Transitioning,
        "ERROR" => State::Failed,
        _ => State::Unknown,
    }
}

fn healthy(status: &str) -> bool {
    matches!(
        status.to_ascii_uppercase().as_str(),
        "ONLINE" | "OFFLINE" | "HEALTHY" | "UNHEALTHY"
    )
}

fn member_state(status: &str) -> State {
    if healthy(status) {
        State::Active
    } else if state(status) == State::Active {
        State::Unknown
    } else {
        state(status)
    }
}

fn member(group: &Group) -> Option<&Member> {
    match group.members.as_slice() {
        [member] => Some(member),
        _ => None,
    }
}

struct Chain {
    pool: PoolWire,
    group: Group,
}

impl Chain {
    fn state(&self) -> State {
        [
            state(&self.pool.status),
            self.pool
                .health
                .as_ref()
                .map_or(State::Unknown, |h| state(&h.status)),
            member_state(&self.group.status),
            member(&self.group).map_or(State::Unknown, |m| member_state(&m.status)),
        ]
        .into_iter()
        .max()
        .unwrap_or(State::Unknown)
    }
}

fn safe_id(id: &str, evidence: &Evidence) -> String {
    if evidence.id_reflects(id) {
        "<redacted>".into()
    } else {
        id.into()
    }
}
