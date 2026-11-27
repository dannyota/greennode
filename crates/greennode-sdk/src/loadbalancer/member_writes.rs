use std::net::Ipv4Addr;

use bytes::Bytes;
use http::{Method, StatusCode};
use serde::{Serialize, Serializer, ser::SerializeStruct};

use crate::wire::product::{evidence::Evidence, redirect::Retry};
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use super::child_writes::{
    BOUND, BusyRefusal, ChildTarget, ChildWriter, CompleteRefusal, WriteResult, encode_body,
    not_sent,
};
use super::deletes::{context_error, local, safe_text};
use super::member_state::{self, Entry, MEMBERS, default_port, default_weight, key};
use super::pool_output::PoolProjection;
use super::pool_writes::{pool_target, settle_until};
use super::wire::{DecodeError, invalid_input};
use super::{Client, Pool, WriteError};

const ADD: &str = "loadbalancer.AddPoolMember";
const UPDATE: &str = "loadbalancer.UpdatePoolMember";
const REMOVE: &str = "loadbalancer.RemovePoolMember";

/// Adds a member to a load balancer pool.
#[derive(Clone, Debug, Default, Serialize)]
pub struct AddPoolMemberInput {
    /// Parent load balancer identifier.
    #[serde(skip)]
    pub load_balancer_id: String,
    /// Pool identifier.
    #[serde(skip)]
    pub pool_id: String,
    /// Member IPv4 address.
    pub address: String,
    /// Member traffic port.
    pub port: i64,
    /// Member name.
    pub name: String,
    /// Weight used when balancing traffic.
    pub weight: i64,
    /// Health check port. Zero uses the traffic port.
    pub monitor_port: i64,
    /// Whether this member serves as a backup.
    pub backup: bool,
    /// Returns after the provider accepts the replacement.
    #[serde(skip)]
    pub no_wait: bool,
}

/// Changes an existing load balancer pool member.
#[derive(Clone, Debug, Default, Serialize)]
pub struct UpdatePoolMemberInput {
    /// Parent load balancer identifier.
    #[serde(skip)]
    pub load_balancer_id: String,
    /// Pool identifier.
    #[serde(skip)]
    pub pool_id: String,
    /// Member IPv4 address.
    pub address: String,
    /// Member traffic port.
    pub port: i64,
    /// Member name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Weight used when balancing traffic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<i64>,
    /// Health check port. Zero uses the traffic port.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor_port: Option<i64>,
    /// Whether this member serves as a backup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<bool>,
    /// Returns after the provider accepts the replacement.
    #[serde(skip)]
    pub no_wait: bool,
}

/// Identifies a load balancer pool member to remove.
#[derive(Clone, Debug, Default, Serialize)]
pub struct RemovePoolMemberInput {
    /// Parent load balancer identifier.
    #[serde(skip)]
    pub load_balancer_id: String,
    /// Pool identifier.
    #[serde(skip)]
    pub pool_id: String,
    /// Member IPv4 address.
    pub address: String,
    /// Member traffic port.
    pub port: i64,
    /// Returns after the provider accepts the replacement.
    #[serde(skip)]
    pub no_wait: bool,
}

macro_rules! member_output {
    ($(#[$meta:meta])* $output:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq)]
        pub struct $output {
            /// The observed or submitted pool.
            pub pool: Pool,
            /// Whether the operation changed the member list.
            pub changed: bool,
            projection: Option<PoolProjection>,
            redact_changed: bool,
        }

        crate::wire::product::output::public_fields!($output { pool, changed; projection, redact_changed });

        impl Serialize for $output {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut output = serializer.serialize_struct(stringify!($output), 2)?;
                if let Some(projection) = &self.projection {
                    output.serialize_field("Pool", projection)?;
                } else {
                    output.serialize_field("Pool", &self.pool)?;
                }
                if self.redact_changed {
                    output.serialize_field("Changed", "<redacted>")?;
                } else {
                    output.serialize_field("Changed", &self.changed)?;
                }
                output.end()
            }
        }
    };
}

member_output!(
    /// Reports the pool after adding a member.
    AddPoolMemberOutput
);
member_output!(
    /// Reports the pool after updating a member.
    UpdatePoolMemberOutput
);
member_output!(
    /// Reports the pool after removing a member.
    RemovePoolMemberOutput
);

struct Selection<'a> {
    load_balancer: &'a str,
    pool: &'a str,
    address: &'a str,
    port: i64,
    no_wait: bool,
}

enum Mutation<'a> {
    Add(&'a AddPoolMemberInput),
    Update(&'a UpdatePoolMemberInput),
    Remove,
}

struct MemberResult {
    pool: Pool,
    changed: bool,
    projection: Option<PoolProjection>,
    redact_changed: bool,
}

#[derive(Serialize)]
struct Replacement<'a> {
    members: &'a [Entry],
}

macro_rules! member_method {
    ($(#[$meta:meta])* $method:ident, $input:ty, $output:ident, $operation:expr, $mutation:expr) => {
        $(#[$meta])*
        pub async fn $method(&self, ctx: &RequestContext, input: $input) -> WriteResult<$output> {
            crate::wire::product::exit::finish!(async {
                let selection = Selection {
                    load_balancer: &input.load_balancer_id, pool: &input.pool_id, address: &input.address,
                    port: input.port, no_wait: input.no_wait,
                };
                self.change_members(ctx, $operation, selection, &input, ($mutation)(&input), |result| $output {
                    pool: result.pool, changed: result.changed, projection: result.projection, redact_changed: result.redact_changed,
                }).await
            })
        }
    };
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    member_method!(
        /// Adds one member and preserves others in the same pool.
        add_pool_member,
        AddPoolMemberInput,
        AddPoolMemberOutput,
        ADD,
        Mutation::Add
    );
    member_method!(
        /// Updates the member selected by address and port.
        update_pool_member,
        UpdatePoolMemberInput,
        UpdatePoolMemberOutput,
        UPDATE,
        Mutation::Update
    );
    member_method!(
        /// Removes the member selected by address and port.
        remove_pool_member,
        RemovePoolMemberInput,
        RemovePoolMemberOutput,
        REMOVE,
        |_| Mutation::Remove
    );

    async fn change_members<O>(
        &self,
        ctx: &RequestContext,
        operation: &'static str,
        selection: Selection<'_>,
        input: &impl Serialize,
        mutation: Mutation<'_>,
        output: impl FnOnce(MemberResult) -> O,
    ) -> WriteResult<O> {
        selection.required(operation).map_err(not_sent)?;
        let ids = [
            ("LoadBalancerID", selection.load_balancer),
            ("PoolID", selection.pool),
        ];
        let mut write = ChildWriter::new(self, ctx, operation, selection.load_balancer, &ids)
            .map_err(not_sent)?;
        write.report_redirect = true;
        encode_body(input, &write.evidence).map_err(not_sent)?;
        selection.validate(operation).map_err(not_sent)?;
        mutation.validate(operation).map_err(not_sent)?;
        let _lock = self
            .lock_load_balancer(ctx, selection.load_balancer)
            .await
            .map_err(|error| not_sent(Box::new(error)))?;
        let clock = self.session.clock();
        let pre = ctx.child_with_deadline(clock.monotonic_now() + BOUND);
        let target = pool_target(selection.pool, &ids);
        let route = ChildTarget {
            read: &MEMBERS,
            ..pool_target(selection.pool, &ids)
        };
        let mut previous = WriteDispatch::NotSent;
        loop {
            let prepared = prepare(ctx, &pre, &mut write, &target).await;
            let (mut pool, members) =
                prepared.map_err(|error| Failure::new(*error, None, previous))?;
            let mut entries: Vec<_> = members.iter().map(Entry::from).collect();
            let changed = mutation
                .apply(&selection, &write, &mut entries)
                .map_err(|error| Failure::new(*error, None, previous))?;
            if !changed {
                return finish(&write, &target, pool, false, Ok(()), previous, output);
            }
            let encoded = encode_body(&Replacement { members: &entries }, &write.evidence)
                .map_err(|error| Failure::new(*error, None, previous))?;
            let sent = send_replacement(ctx, &pre, &mut write, &route, encoded)
                .await
                .map_err(|error| Failure::new(*error, None, previous))?;
            if let Err(failure) = &sent
                && previous == WriteDispatch::NotSent
                && failure.error().detail::<BusyRefusal>().is_some()
            {
                previous = failure.write_dispatch();
                continue;
            }
            let (result, dispatch) = match sent {
                Ok(_) => {
                    let result = if selection.no_wait {
                        member_state::submitted(&mut pool, members, entries);
                        Ok(())
                    } else {
                        confirm(ctx, &mut write, &target, &mut pool, entries).await
                    };
                    (result, WriteDispatch::MayHaveBeenSent)
                }
                Err(failure) => {
                    let (error, _, dispatch) = failure.into_parts();
                    let error = if dispatch == WriteDispatch::MayHaveBeenSent
                        && error.detail::<CompleteRefusal>().is_none()
                    {
                        write.unconfirmed(&target, error)
                    } else {
                        Box::new(error)
                    };
                    let observed = if previous == WriteDispatch::MayHaveBeenSent {
                        previous
                    } else {
                        dispatch
                    };
                    (Err(error), observed)
                }
            };
            return finish(&write, &target, pool, true, result, dispatch, output);
        }
    }
}

impl Selection<'_> {
    fn required(&self, operation: &str) -> Result<(), Box<Error>> {
        let missing = [
            ("LoadBalancerID", self.load_balancer.is_empty()),
            ("PoolID", self.pool.is_empty()),
            ("Address", self.address.is_empty()),
            ("Port", self.port == 0),
        ];
        match missing.iter().find(|(_, empty)| *empty) {
            Some((field, _)) => Err(invalid_input(&format!("{operation} requires {field}"))),
            None => Ok(()),
        }
    }

    fn validate(&self, operation: &str) -> Result<(), Box<Error>> {
        if self.address.parse::<Ipv4Addr>().is_err() {
            return Err(invalid_input(&format!(
                "{operation}: Address must be an IPv4 address, got {:?}",
                self.address
            )));
        }
        check_port(operation, "Port", self.port)
    }
}

impl Mutation<'_> {
    fn validate(&self, operation: &str) -> Result<(), Box<Error>> {
        let monitor = match self {
            Self::Add(input) => input.monitor_port,
            Self::Update(input) => {
                if input.name.is_none()
                    && input.weight.is_none()
                    && input.monitor_port.is_none()
                    && input.backup.is_none()
                {
                    return Err(invalid_input(&format!(
                        "{operation} requires at least one field to change"
                    )));
                }
                input.monitor_port.unwrap_or_default()
            }
            Self::Remove => 0,
        };
        if monitor != 0 {
            check_port(operation, "MonitorPort", monitor)?;
        }
        Ok(())
    }

    fn apply(
        &self,
        selection: &Selection<'_>,
        write: &ChildWriter<'_>,
        entries: &mut Vec<Entry>,
    ) -> Result<bool, Box<Error>> {
        let target = key(selection.address, selection.port);
        let found = entries.iter().position(|entry| entry.key() == target);
        if let Self::Add(input) = self {
            let proposed = Entry {
                ip_address: selection.address.into(),
                port: selection.port,
                backup: input.backup,
                weight: default_weight(input.weight),
                name: input.name.clone(),
                monitor_port: default_port(selection.port, input.monitor_port),
            };
            if let Some(current) = found.and_then(|index| entries.get(index)) {
                if current.same_settings(&proposed) {
                    return Ok(false);
                }
                let location = member_location(selection, &write.evidence);
                let message = format!(
                    "{}: pool {} already has a member at {location}; use update-pool-member to change it",
                    write.operation,
                    safe_text(selection.pool, &write.evidence)
                );
                return Err(Box::new(local(
                    ErrorClass::InvalidInput,
                    &format!("vngcloud: invalid input: {message}"),
                    &write.evidence,
                )));
            }
            entries.push(proposed);
            return Ok(true);
        }
        let Some(index) = found else {
            let location = member_location(selection, &write.evidence);
            let message = format!(
                "vngcloud: resource not found: {}: pool {} has no member at {location}",
                write.operation,
                safe_text(selection.pool, &write.evidence)
            );
            return Err(Box::new(local(
                ErrorClass::NotFound,
                &message,
                &write.evidence,
            )));
        };
        if let Self::Update(input) = self {
            let Some(current) = entries.get_mut(index) else {
                return Err(Box::new(Error::new(ErrorClass::NotFound)));
            };
            let updated = Entry {
                name: input.name.clone().unwrap_or_else(|| current.name.clone()),
                weight: input.weight.map_or(current.weight, default_weight),
                monitor_port: input.monitor_port.map_or(current.monitor_port, |monitor| {
                    default_port(current.port, monitor)
                }),
                backup: input.backup.unwrap_or(current.backup),
                ..current.clone()
            };
            if current.same_settings(&updated) {
                return Ok(false);
            }
            *current = updated;
        } else {
            entries.remove(index);
        }
        Ok(true)
    }
}

fn member_location(selection: &Selection<'_>, evidence: &Evidence) -> String {
    format!(
        "{}:{}",
        safe_text(selection.address, evidence),
        selection.port
    )
}

fn check_port(operation: &str, field: &str, port: i64) -> Result<(), Box<Error>> {
    if (1..=65_535).contains(&port) {
        return Ok(());
    }
    Err(invalid_input(&format!(
        "{operation}: {field} must be 1 to 65535, got {port}"
    )))
}

async fn prepare(
    ctx: &RequestContext,
    pre: &RequestContext,
    write: &mut ChildWriter<'_>,
    target: &ChildTarget<'_>,
) -> Result<(Pool, Vec<super::PoolMember>), Box<Error>> {
    write.ready_time(ctx, pre, Some(target))?;
    let ready = write
        .ready_with(pre, Some(target), member_state::validate_pool_document)
        .await;
    write.ready_time(ctx, pre, Some(target))?;
    let ready = ready?;
    let (pool, embedded) = member_state::pool_and_embedded(
        ready.child.as_deref().unwrap_or("null"),
        target.read,
        &write.evidence,
    )?;
    let raw = write.read_data(pre, &MEMBERS, target.ids).await;
    write.ready_time(ctx, pre, Some(target))?;
    let members = member_state::read_members(&raw?, &write.evidence)?;
    member_state::check_agree(write, target, &pool, embedded, &members)?;
    Ok((pool, members))
}

pub(super) async fn send_replacement(
    ctx: &RequestContext,
    pre: &RequestContext,
    write: &mut ChildWriter<'_>,
    route: &ChildTarget<'_>,
    encoded: Bytes,
) -> Result<Result<(StatusCode, Bytes), Failure<()>>, Box<Error>> {
    preflight_time(ctx, pre, write)?;
    Ok(write
        .send_once::<()>(ctx, &Method::PUT, route, encoded, Retry::Once)
        .await)
}

fn preflight_time(
    ctx: &RequestContext,
    pre: &RequestContext,
    write: &ChildWriter<'_>,
) -> Result<(), Box<Error>> {
    let clock = write.client.session.clock();
    if let Some(end) = ctx.check(clock) {
        return Err(Box::new(context_error(end)));
    }
    if pre.check(clock).is_some() {
        let message = format!(
            "loadbalancer: resource busy: {}: pool is not ready; nothing sent",
            write.operation
        );
        return Err(Box::new(
            local(ErrorClass::Api, &message, &write.evidence)
                .with_outer_detail(WriteError::ResourceBusy),
        ));
    }
    Ok(())
}

async fn confirm(
    ctx: &RequestContext,
    write: &mut ChildWriter<'_>,
    target: &ChildTarget<'_>,
    pool: &mut Pool,
    entries: Vec<Entry>,
) -> Result<(), Box<Error>> {
    let wait = ctx.child_with_deadline(write.client.session.clock().monotonic_now() + BOUND);
    settle_until(ctx, &wait, write, target, false, pool).await?;
    write.confirm_time(ctx, &wait, target)?;
    let raw = write.read_data(&wait, &MEMBERS, target.ids).await;
    write.confirm_time(ctx, &wait, target)?;
    let members = raw
        .and_then(|raw| member_state::read_members(&raw, &write.evidence))
        .map_err(|error| write.unconfirmed(target, *error))?;
    let equal = member_state::matches_replacement(&members, entries);
    write.confirm_time(ctx, &wait, target)?;
    if equal {
        return Ok(());
    }
    let pool = safe_text(target.id, &write.evidence);
    let message = format!(
        "vngcloud: write outcome not confirmed: {}: pool {pool}: members read after the write do not match what was sent; another writer may have changed the pool",
        write.operation
    );
    Err(Box::new(local(
        ErrorClass::NotSettled,
        &message,
        &write.evidence,
    )))
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
fn finish<O>(
    write: &ChildWriter<'_>,
    target: &ChildTarget<'_>,
    pool: Pool,
    changed: bool,
    result: Result<(), Box<Error>>,
    dispatch: WriteDispatch,
    output: impl FnOnce(MemberResult) -> O,
) -> WriteResult<O> {
    let retained = retain(pool, changed, &write.evidence).map(output);
    match (result, retained) {
        (Ok(()), Ok(output)) => Ok(output),
        (Ok(()), Err(())) => Err(Failure::new(
            *target
                .read
                .decode_error(&write.evidence, DecodeError::ReflectedCredential),
            None,
            dispatch,
        )),
        (Err(error), retained) => {
            let partial = if write.evidence.can_retain_partial() {
                retained.ok()
            } else {
                None
            };
            Err(Failure::new(*error, partial, dispatch))
        }
    }
}

fn retain(mut pool: Pool, changed: bool, evidence: &Evidence) -> Result<MemberResult, ()> {
    let redact_changed = evidence.text_reflects(if changed { "true" } else { "false" });
    if evidence.text_reflects("Changed")
        || (redact_changed
            && (evidence.text_reflects("false") || evidence.text_reflects("<redacted>")))
    {
        return Err(());
    }
    let projection = PoolProjection::retain(&mut pool, evidence)?;
    Ok(MemberResult {
        pool,
        changed: changed && !redact_changed,
        projection,
        redact_changed,
    })
}
