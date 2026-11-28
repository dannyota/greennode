use std::collections::BTreeMap;
use std::net::IpAddr;

use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::json;

use crate::network::{Route, RouteTable};
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use super::super::super::Client;
use super::models::*;
use super::wire::{
    BOUND, INTERVAL, NetworkWriteError, Request, Write, input, is_4xx, not_sent, prefix, sent,
};

const ADD: Request = Request::new(
    "network.AddRoute",
    "v2/{}/route-table/{}/routes",
    &Method::PUT,
    &[StatusCode::OK],
)
.once()
.withheld("route response withheld");
const REMOVE: Request = Request::new(
    "network.RemoveRoute",
    "v2/{}/route-table/{}/routes",
    &Method::PUT,
    &[StatusCode::OK],
)
.once()
.withheld("route response withheld");

#[derive(Serialize)]
struct Entry {
    #[serde(rename = "destinationCidrBlock")]
    destination: String,
    target: String,
}

impl From<&Route> for Entry {
    fn from(route: &Route) -> Self {
        Self {
            destination: route.destination_cidr_block.clone(),
            target: route.target.clone(),
        }
    }
}

impl From<Entry> for Route {
    fn from(entry: Entry) -> Self {
        Self {
            destination_cidr_block: entry.destination,
            target: entry.target,
            ..Self::default()
        }
    }
}

impl Entry {
    fn key(&self) -> (String, String) {
        (
            destination(&self.destination),
            canonical_target(&self.target),
        )
    }
}

fn destination(value: &str) -> String {
    prefix(value).map_or_else(|| value.to_owned(), |(ip, bits, _)| format!("{ip}/{bits}"))
}

fn canonical_target(value: &str) -> String {
    value
        .parse::<IpAddr>()
        .map_or_else(|_| value.to_owned(), |ip| ip.to_string())
}

fn counts(entries: impl Iterator<Item = (String, String)>) -> BTreeMap<(String, String), usize> {
    let mut result = BTreeMap::new();
    for key in entries {
        *result.entry(key).or_default() += 1;
    }
    result
}

fn same_routes(routes: &[Route], entries: &[Entry]) -> bool {
    counts(routes.iter().map(Entry::from).map(|entry| entry.key()))
        == counts(entries.iter().map(Entry::key))
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Client {
    /// Adds a route after checking for an existing entry and concurrent changes.
    pub async fn add_route(
        &self,
        ctx: &RequestContext,
        input: AddRouteInput,
    ) -> Result<AddRouteOutput, Failure<AddRouteOutput>> {
        let mut write = Write::new(self, ctx)?;
        let op = ADD.id;
        let id = &input.route_table_id;
        write.check(
            op,
            &[
                ("DestinationCIDR", &input.destination_cidr),
                ("Target", &input.target),
            ],
            &[("RouteTableID", id)],
        )?;
        let want = route_destination(op, &input.destination_cidr)?;
        let target = input
            .target
            .parse::<IpAddr>()
            .map_err(|_| {
                super::wire::input(op, "requires Target to be an IP address without a zone")
            })?
            .to_string();
        let table = write.active_table(op, id).await.map_err(not_sent)?;
        let mut entries: Vec<_> = table.routes.iter().map(Entry::from).collect();
        if let Some(entry) = entries
            .iter()
            .find(|entry| destination(&entry.destination) == want)
        {
            if canonical_target(&entry.target) == target {
                return write.unchanged(
                    op,
                    AddRouteOutput {
                        route_table: table,
                        changed: false,
                    },
                );
            }
            return Err(not_sent(write.local(ErrorClass::InvalidInput, &format!("vngcloud: invalid input: {op}: route table {id} already has a route to {want} with target {}; remove it first", entry.target))));
        }
        entries.push(Entry {
            destination: want,
            target,
        });
        write
            .replace_routes(ADD, id, table, entries, input.no_wait, |route_table| {
                AddRouteOutput {
                    route_table,
                    changed: true,
                }
            })
            .await
    }

    /// Removes the single route matching the destination prefix.
    pub async fn remove_route(
        &self,
        ctx: &RequestContext,
        input: RemoveRouteInput,
    ) -> Result<RemoveRouteOutput, Failure<RemoveRouteOutput>> {
        let mut write = Write::new(self, ctx)?;
        let op = REMOVE.id;
        let id = &input.route_table_id;
        write.check(
            op,
            &[("DestinationCIDR", &input.destination_cidr)],
            &[("RouteTableID", id)],
        )?;
        let want = route_destination(op, &input.destination_cidr)?;
        let table = write.active_table(op, id).await.map_err(not_sent)?;
        let entries: Vec<_> = table
            .routes
            .iter()
            .filter(|route| destination(&route.destination_cidr_block) != want)
            .map(Entry::from)
            .collect();
        match table.routes.len() - entries.len() {
            0 => return Err(not_sent(write.local(ErrorClass::NotFound, &format!("vngcloud: resource not found: {op}: route table {id} has no route to {want}")))),
            1 => {},
            count => return Err(not_sent(write.local(ErrorClass::InvalidInput, &format!("vngcloud: invalid input: {op}: route table {id} has {count} routes to {want}; remove is refused rather than guessing which to drop")))),
        }
        write
            .replace_routes(REMOVE, id, table, entries, input.no_wait, |route_table| {
                RemoveRouteOutput {
                    route_table,
                    changed: true,
                }
            })
            .await
    }
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
fn route_destination<T>(op: &'static str, value: &str) -> Result<String, Failure<T>> {
    match prefix(value) {
        Some((ip, bits, true)) => Ok(format!("{ip}/{bits}")),
        Some(_) => Err(input(
            op,
            "requires DestinationCIDR to have no host bits set",
        )),
        None => Err(input(op, "requires DestinationCIDR to be a CIDR prefix")),
    }
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Write<'_> {
    async fn active_table(&mut self, op: &'static str, id: &str) -> Result<RouteTable, Error> {
        self.active(
            op,
            &format!("route table {id}"),
            async |write, ctx| write.table(ctx, id).await,
            |table| &table.status,
        )
        .await
    }

    async fn replace_routes<O: Serialize>(
        &mut self,
        request: Request,
        id: &str,
        mut base: RouteTable,
        entries: Vec<Entry>,
        no_wait: bool,
        output: impl FnOnce(RouteTable) -> O,
    ) -> Result<O, Failure<O>> {
        let op = request.id;
        let recheck = self.table(self.ctx, id).await.map_err(not_sent)?;
        let previous: Vec<_> = base.routes.iter().map(Entry::from).collect();
        if !same_routes(&recheck.routes, &previous) {
            return Err(not_sent(self.guard_error(NetworkWriteError::Busy, &format!("{op}: route table {id} changed since it was read; nothing sent, run the call again"))));
        }
        if let Err(failure) = self
            .send(request, &[id], Some(json!({"routes": entries})))
            .await
        {
            if failure.write_dispatch() == WriteDispatch::NotSent || is_4xx(failure.error()) {
                return Err(failure);
            }
            let (error, _, _) = failure.into_parts();
            let mut observed = None;
            let confirmation = if super::wire::can_confirm(&error) {
                self.confirm_routes(op, id, &entries, &mut observed)
                    .await
                    .err()
            } else {
                None
            };
            if let Some(row) = observed {
                base = row;
            }
            let error = self.settled_error_with_confirmation(
                &format!("{op}: routes were sent once; read the route table before another write"),
                error,
                confirmation,
            );
            return self.outcome(op, base, Err(error), output);
        }
        if no_wait {
            base.routes = entries.into_iter().map(Route::from).collect();
            return self.outcome(op, base, Ok(()), output);
        }
        let mut observed = None;
        let result = self.confirm_routes(op, id, &entries, &mut observed).await;
        if let Some(table) = observed {
            self.outcome(op, table, result, output)
        } else {
            Err(sent(result.err().unwrap_or_else(|| self.invalid(op)), None))
        }
    }

    async fn confirm_routes(
        &mut self,
        op: &'static str,
        id: &str,
        entries: &[Entry],
        observed: &mut Option<RouteTable>,
    ) -> Result<(), Error> {
        let resource = format!("route table {id}");
        self.settle(BOUND, INTERVAL,
            &format!("{op}: {resource} did not settle within 1m0s; the write may have reached the server and is safe to run again"),
            async |write, ctx| {
                let row = match write.table(ctx, id).await {
                    Ok(row) => row,
                    Err(error) if error.has_class(ErrorClass::NotFound) => return Ok(None),
                    Err(error) => return Err(write.settled_error(&format!("{op}: {resource}"), error)),
                };
                let status = row.status.clone();
                let matches = same_routes(&row.routes, entries);
                *observed = Some(row);
                match status.as_str() {
                    "ERROR" => Err(write.status_error(op, &resource)),
                    "ACTIVE" if matches => Ok(Some(())),
                    "ACTIVE" => Err(write.local(ErrorClass::NotSettled, &format!("vngcloud: write outcome not confirmed: {op}: {resource}: routes read after the write do not match what was sent; another writer may have changed the table"))),
                    _ => Ok(None),
                }
            }).await
    }
}
