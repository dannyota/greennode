mod catalog;
mod cidr;
mod create;
mod delete;
mod model;
mod types;

pub use types::*;

use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::{Value, value::RawValue};
use std::time::Duration;

use super::load_balancer_writes::decode;
use super::wire::{DecodeError, WireField};
use super::write::{
    PROVENANCE, Settlement, Write, guard_operation, local, not_sent, rejected_response,
    write_operation,
};
use super::{Client, invalid_input};
use crate::endpoint::Base;
use crate::quote::{catalog::unique, request};
use crate::wire::product::{evidence::Evidence, options::PathIds, sender::Response};
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};
use model::{Group, ListenerWire, Member, Parent, PoolWire};

const POOLS: &str = "v1/global-load-balancers/{}/global-pools";
const POOL: &str = "v1/global-load-balancers/{}/global-pools/{}";
const LISTENERS: &str = "v1/global-load-balancers/{}/global-listeners";
const LISTENER: &str = "v1/global-load-balancers/{}/global-listeners/{}";
const GROUPS: &str = "v1/global-load-balancers/{}/global-pools/{}/pool-members";
const GROUP: &str = "v1/global-load-balancers/{}/global-pools/{}/pool-members/{}";
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
        if self.0.evidence.reflected_in(values) {
            Err(refuse("a request value contains a credential"))
        } else {
            Ok(())
        }
    }

    async fn get<T: WireField>(
        &mut self,
        ctx: &RequestContext,
        template: &'static str,
        path: &[&str],
    ) -> Result<T, Box<Error>> {
        self.get_from(ctx, template, path, false).await
    }

    async fn get_from<T: WireField>(
        &mut self,
        ctx: &RequestContext,
        template: &'static str,
        path: &[&str],
        regional: bool,
    ) -> Result<T, Box<Error>> {
        let op = guard_operation(self.0.id, template);
        let session = &self.0.client.session;
        let base = if regional {
            request::Product::Regional.endpoint(session)
        } else {
            session.endpoints().global_load_balancer.as_ref()
        };
        let reply = self
            .0
            .request_on(ctx, &op, base, path, None)
            .await
            .map_err(|failure| {
                let error = failure.into_parts().0;
                if error
                    .api_error()
                    .is_some_and(|api| api.operation() == self.0.id && api.status().is_none())
                    && !error.has_identity(ErrorClass::InvalidInput)
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
            let classified = rejected_response(&op, &reply, None, Some(PROVIDER_TEXT));
            let absent = reply.status == StatusCode::NOT_FOUND || internal_error(&reply);
            return Err(Box::new(if absent {
                classified.with_detail(Absent)
            } else {
                classified
            }));
        }
        if reply.body.is_empty() || super::wire::go_json(&reply.body).trim() == "null" {
            return Err(self.invalid(DecodeError::InvalidShape));
        }
        let result: T = decode(&op, &reply.body, &T::SHAPE).map_err(|error| {
            self.invalid(
                error
                    .detail::<DecodeError>()
                    .copied()
                    .unwrap_or(DecodeError::InvalidShape),
            )
        })?;
        if !self.clean_document(&reply.body) {
            return Err(self.invalid(DecodeError::ReflectedCredential));
        }
        Ok(result)
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
            Err(failure) => Err(self
                .0
                .rejection(failure, partial(self), "request unconfirmed")),
        }
    }

    fn acceptance(&self, reply: &Response, template: &'static str) -> Option<model::Acceptance> {
        let op = guard_operation(self.0.id, template);
        let result = decode(&op, &reply.body, &model::Acceptance::SHAPE).ok()?;
        if !self.clean_document(&reply.body) {
            None
        } else {
            Some(result)
        }
    }

    fn clean_document(&self, body: &[u8]) -> bool {
        let text = super::wire::go_json(body);
        let Ok(raw) = serde_json::from_str::<&RawValue>(&text) else {
            return false;
        };
        crate::quote::catalog::value(self.0.id, raw)
            .is_ok_and(|value| !model::reflected_text(&value, &self.0.evidence))
    }

    fn fail<O: Serialize>(&self, reason: &'static str, partial: O) -> Failure<O> {
        self.0.finish(Settlement::Unknown(reason), Some(partial))
    }

    async fn parent(&mut self, ctx: &RequestContext, glb: &str) -> Result<Parent, Box<Error>> {
        let parent: Parent = self.get(ctx, PARENT, &[glb]).await?;
        if parent.id != glb
            || parent.kind != "Layer 4"
            || state(&parent.status) != State::Active
            || parent.package.is_empty()
        {
            return Err(refuse(
                "the load balancer is not an eligible Layer 4 load balancer",
            ));
        }
        Ok(parent)
    }

    async fn pools(
        &mut self,
        ctx: &RequestContext,
        glb: &str,
    ) -> Result<Vec<PoolWire>, Box<Error>> {
        let rows: Vec<PoolWire> = self.get(ctx, POOLS, &[glb]).await?;
        self.unique(
            rows.iter().map(|row| row.id.as_str()),
            PathIds::WithUnderscore,
        )?;
        Ok(rows)
    }

    async fn listeners(
        &mut self,
        ctx: &RequestContext,
        glb: &str,
    ) -> Result<Vec<ListenerWire>, Box<Error>> {
        let rows: Vec<ListenerWire> = self.get(ctx, LISTENERS, &[glb]).await?;
        self.unique(
            rows.iter().map(|row| row.id.as_str()),
            PathIds::WithUnderscore,
        )?;
        Ok(rows)
    }

    async fn groups(
        &mut self,
        ctx: &RequestContext,
        glb: &str,
        pool: &str,
    ) -> Result<Vec<Group>, Box<Error>> {
        let rows: Vec<Group> = self.get(ctx, GROUPS, &[glb, pool]).await?;
        self.unique(
            rows.iter().map(|row| row.id.as_str()),
            PathIds::WithUnderscore,
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
        if self.listeners(ctx, glb).await?.is_empty() {
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
    ) -> Result<PoolWire, Box<Error>> {
        let pool: PoolWire = self.get(ctx, POOL, &[glb, id]).await?;
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
        Ok(pool)
    }

    async fn exact_group(
        &mut self,
        ctx: &RequestContext,
        glb: &str,
        pool: &str,
        group: &str,
    ) -> Result<Group, Box<Error>> {
        if !PathIds::Strict.allows(group) {
            return Err(self.invalid(DecodeError::InvalidShape));
        }
        let row: Group = self.get(ctx, GROUP, &[glb, pool, group]).await?;
        if row.id != group
            || row.pool != pool
            || row.glb != glb
            || row.kind != PRIVATE
            || row.traffic_dial != TRAFFIC
            || member(&row).is_none_or(|m| {
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
        Ok(row)
    }

    async fn chain(
        &mut self,
        ctx: &RequestContext,
        glb: &str,
        pool: &str,
        group: Option<&str>,
        name: &str,
    ) -> Result<Chain, Box<Error>> {
        if !matches!(self.pools(ctx, glb).await?.as_slice(), [row] if row.id == pool) {
            return Err(refuse(
                "the load balancer must hold exactly the expected pool",
            ));
        }
        let pool = self.exact_pool(ctx, glb, pool, name).await?;
        let rows = self.groups(ctx, glb, &pool.id).await?;
        let [row] = rows.as_slice() else {
            return Err(refuse(
                "the pool must hold exactly the expected member group",
            ));
        };
        if group.is_some_and(|id| row.id != id) {
            return Err(refuse(
                "the pool must hold exactly the expected member group",
            ));
        }
        let group = self.exact_group(ctx, glb, &pool.id, &row.id).await?;
        Ok(Chain { pool, group })
    }
}

struct Absent;

fn internal_error(reply: &Response) -> bool {
    if reply.status != StatusCode::INTERNAL_SERVER_ERROR {
        return false;
    }
    let op = guard_operation("", "");
    let shape = crate::wire::strict_json::Shape::Object(&[crate::wire::strict_json::ShapeField {
        name: "code",
        shape: crate::wire::strict_json::Shape::Any,
    }]);
    #[derive(Default, serde::Deserialize)]
    struct Code {
        code: Option<String>,
    }
    decode::<Code>(&op, &reply.body, &shape)
        .is_ok_and(|envelope| envelope.code.as_deref() == Some("internal_error"))
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
    if evidence.text_reflects(id) {
        "<redacted>".into()
    } else {
        id.into()
    }
}
