use serde::Serialize;

use crate::model::wire::{WireField, go_json, model};
use crate::wire::product::options::PathIds;
use crate::{Failure, List, PagedList, RequestContext};

use super::route_inventory::{
    CAP, InventoryRead, ResponseKind, field, ids_reflect, paged_input, required_id_input,
};
use super::{Client, failure};

const ROUTE_TABLE: InventoryRead = InventoryRead::new(
    "network.GetRouteTable",
    "v2/{}/route-table/{}",
    ResponseKind::RouteTable,
);
const ROUTE_TABLES: InventoryRead =
    InventoryRead::standard("network.ListRouteTables", "v2/{}/route-table");

required_id_input!(
    /// Selects a route table to read.
    GetRouteTableInput,
    route_table_id
);

/// The selected route table, including its routes.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetRouteTableOutput {
    /// Route table settings and routes.
    #[serde(rename = "RouteTable")]
    pub route_table: RouteTable,
}

paged_input!(
    /// Selects a page of route tables.
    ListRouteTablesInput
);

/// One page of route tables.
pub type ListRouteTablesOutput = PagedList<RouteTable>;

/// Lists the routes from the default page of route tables.
#[derive(Clone, Debug, Default)]
pub struct ListRouteTableRoutesInput {}

/// Flattened routes from the default page of route tables, in provider order.
pub type ListRouteTableRoutesOutput = List<RouteTableRoute>;

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl Client {
    /// Reads the selected route table and its routes.
    pub async fn get_route_table(
        &self,
        ctx: &RequestContext,
        input: GetRouteTableInput,
    ) -> Result<GetRouteTableOutput, Failure<GetRouteTableOutput>> {
        let id = &input.route_table_id;
        let mut evidence = self.request_evidence(ctx).map_err(failure)?;
        ROUTE_TABLE
            .check_id("RouteTableID", id, PathIds::Strict, &evidence)
            .map_err(failure)?;
        let project = self
            .inventory_project(ctx, &[id], &mut evidence)
            .await
            .map_err(failure)?;
        let base = self.vserver_inventory_base().map_err(failure)?;
        let raw = self
            .read_route_inventory(ctx, base, ROUTE_TABLE, &[&project, id], &[], &mut evidence)
            .await
            .map_err(failure)?;
        let text = go_json(&raw);
        if evidence.go_peer_body_reflects(&raw) == Some(true) {
            return Err(failure(ROUTE_TABLE.invalid(&evidence)));
        }
        let envelope = ROUTE_TABLE.fields(&text, &evidence).map_err(failure)?;
        let invalid = |()| failure(ROUTE_TABLE.invalid(&evidence));
        let incomplete = || failure(ROUTE_TABLE.incomplete(&evidence));
        let mut table: RouteTable = field(&envelope, "data").map_err(|()| incomplete())?;
        let fields = envelope
            .output_raw("data")
            .map_err(|()| incomplete())?
            .and_then(|raw| {
                serde_json::from_str::<Option<super::wire::Fields<'_>>>(raw.get())
                    .map_err(|_| ())
                    .transpose()
            })
            .ok_or_else(incomplete)?
            .map_err(|()| incomplete())?;
        if fields
            .output_raw("routes")
            .map_err(|()| incomplete())?
            .is_none_or(|value| value.get() == "null")
            || table.uuid != *id
            || table.status.is_empty()
        {
            return Err(incomplete());
        }
        if ids_reflect(&table, &evidence) || !table.redact(&evidence) {
            return Err(invalid(()));
        }
        Ok(GetRouteTableOutput { route_table: table })
    }

    /// Lists one page of route tables for the selected project.
    pub async fn list_route_tables(
        &self,
        ctx: &RequestContext,
        input: ListRouteTablesInput,
    ) -> Result<ListRouteTablesOutput, Failure<ListRouteTablesOutput>> {
        self.list_route_inventory(ctx, ROUTE_TABLES, (&input.name, input.page, input.size))
            .await
            .map_err(failure)
    }

    /// Flattens routes from the default page of route tables.
    pub async fn list_route_table_routes(
        &self,
        ctx: &RequestContext,
        _input: ListRouteTableRoutesInput,
    ) -> Result<ListRouteTableRoutesOutput, Failure<ListRouteTableRoutesOutput>> {
        let mut evidence = self.request_evidence(ctx).map_err(failure)?;
        let project = self
            .inventory_project(ctx, &[], &mut evidence)
            .await
            .map_err(failure)?;
        let (tables, bytes) = self
            .inventory_page::<RouteTable>(ctx, ROUTE_TABLES, &project, ("", 0, 0), &mut evidence)
            .await
            .map_err(failure)?;
        let copied_ids = tables.items.iter().fold(0usize, |total, table| {
            total.saturating_add(table.uuid.len().saturating_mul(table.routes.len()))
        });
        if bytes.saturating_add(copied_ids) > CAP {
            return Err(failure(ROUTE_TABLES.incomplete(&evidence)));
        }
        let mut items: Vec<_> = tables
            .items
            .into_iter()
            .flat_map(|table| {
                table.routes.into_iter().map(move |route| RouteTableRoute {
                    route_table_id: table.uuid.clone(),
                    uuid: route.uuid,
                    routing_type: route.routing_type,
                    destination_cidr_block: route.destination_cidr_block,
                    target: route.target,
                    status: route.status,
                })
            })
            .collect();
        if ids_reflect(&items, &evidence) || !items.redact(&evidence) {
            return Err(failure(ROUTE_TABLES.invalid(&evidence)));
        }
        Ok(List { items })
    }
}

model! {
    @ true;
    /// One entry in a route table.
    pub struct Route {
        /// Provider resource identifier.
        uuid: String => "uuid", "UUID";
        /// Identifier of the route table.
        route_table_id: String => "routeTableId", "RouteTableID";
        /// Provider routing type.
        routing_type: String => "routingType", "RoutingType";
        /// Destination network in CIDR notation.
        destination_cidr_block: String => "destinationCidrBlock", "DestinationCIDRBlock";
        /// Next hop reported for the route.
        target: String => "target", "Target";
        /// Provider-reported state.
        status: String => "status", "Status";
    }
}

model! {
    @ true;
    /// A routing policy and its provider-reported routes.
    pub struct RouteTable {
        /// Provider resource identifier.
        uuid: String => "uuid", "UUID";
        /// Display name.
        name: String => "name", "Name";
        /// Provider-reported state.
        status: String => "status", "Status";
        /// Network identifier.
        network_id: String => "networkId", "NetworkID";
        /// Creation time as supplied by the service.
        created_at: String => "createdAt", "CreatedAt";
        /// Routes in provider order.
        routes: Vec<Route> => "routes", "Routes";
    }
}

model! {
    @ true;
    /// A route with the identifier of its containing table.
    pub struct RouteTableRoute {
        /// Identifier of the route table.
        route_table_id: String => "routeTableId", "RouteTableID";
        /// Provider resource identifier.
        uuid: String => "uuid", "UUID";
        /// Provider routing type.
        routing_type: String => "routingType", "RoutingType";
        /// Destination network in CIDR notation.
        destination_cidr_block: String => "destinationCidrBlock", "DestinationCIDRBlock";
        /// Next hop reported for the route.
        target: String => "target", "Target";
        /// Provider-reported state.
        status: String => "status", "Status";
    }
}
