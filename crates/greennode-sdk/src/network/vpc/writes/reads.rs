use std::collections::HashSet;
use std::time::Duration;

use serde::Serialize;
use serde_json::value::RawValue;

use crate::model::wire::{Field, WireField, go_json};
use crate::wire::product::{guard::guarded_poll, options::PathIds, output::PublicOutput};
use crate::{DEFAULT_PAGE, DEFAULT_PAGE_SIZE, Error, ErrorClass, RequestContext};

use super::super::super::{
    Client, ElasticNetworkInterface, RouteTable, VirtualIpAddress, connectivity::INTERFACES,
    route_inventory::InventoryRead, routes::ROUTE_TABLE, virtual_ip::VIRTUAL_IPS, wire::Fields,
};
use super::super::{
    GET_DHCP,
    models::{DhcpOptions, NetworkAcl, Subnet, Vpc},
    policy::{GET_ACL, LIST_ACLS},
    subnets::{BY_VPC, GET_SUBNET, GET_VPC, SubnetWire},
    wire::Read,
};
use super::wire::{BOUND, INTERVAL, NetworkWriteError, Write, is_5xx};

const VPC: &[&str] = &[
    "id",
    "status",
    "elasticIps",
    "displayName",
    "createdAt",
    "cidr",
    "dhcpOptionName",
    "dhcpOptionId",
    "routeTableName",
    "routeTableId",
    "zone",
    "dnsStatus",
    "dnsId",
    "mtu",
    "serverCount",
    "volumeCount",
];
const ZONE: &[&str] = &[
    "uuid",
    "name",
    "zoneType",
    "isDefault",
    "description",
    "isEnabled",
    "openstackZone",
    "ipRanges",
    "serverCount",
    "volumeCount",
];
const DHCP: &[&str] = &[
    "uuid",
    "name",
    "status",
    "dnsServers",
    "mtu",
    "associatedNetworks",
    "createdAt",
    "updatedAt",
];
const SUBNET: &[&str] = &[
    "uuid",
    "status",
    "cidr",
    "networkUuid",
    "routeTableUuid",
    "name",
    "interfaceAclPolicyUuid",
    "interfaceAclPolicyName",
    "secondarySubnets",
    "zone",
];
const ACL: &[&str] = &[
    "uuid",
    "id",
    "name",
    "description",
    "status",
    "networkId",
    "subnetId",
    "createdAt",
    "defaultAcl",
    "interfaceNetworkUuid",
    "aclPolicyRules",
    "subnetAssociationList",
];
const ACL_RULE: &[&str] = &[
    "uuid",
    "type",
    "seqNumber",
    "protocol",
    "port",
    "source",
    "action",
    "system",
];
const TABLE: &[&str] = &["uuid", "name", "status", "networkId", "createdAt", "routes"];
const ROUTE: &[&str] = &[
    "uuid",
    "routeTableId",
    "routingType",
    "destinationCidrBlock",
    "target",
    "status",
];
const PAGED: &[&str] = &["listData", "page", "pageSize", "totalPage", "totalItem"];
const MAX_DEPENDENT_PAGES: i64 = 50;

fn exact<'a>(raw: &'a str, keys: &[&str]) -> Result<Fields<'a>, ()> {
    let fields: Fields<'_> = serde_json::from_str(raw).map_err(|_| ())?;
    for key in keys {
        fields.raw(key)?;
    }
    Ok(fields)
}

fn exact_items(raw: &str, parse: fn(&str) -> Result<(), ()>) -> Result<(), ()> {
    for row in serde_json::from_str::<Vec<&RawValue>>(raw).map_err(|_| ())? {
        parse(row.get())?;
    }
    Ok(())
}

fn nested_items(
    fields: &Fields<'_>,
    key: &str,
    parse: fn(&str) -> Result<(), ()>,
) -> Result<(), ()> {
    if let Some(raw) = fields.raw(key)?.filter(|raw| raw.get() != "null") {
        exact_items(raw.get(), parse)?;
    }
    Ok(())
}

fn subnet_shape(raw: &str) -> Result<(), ()> {
    let fields = exact(raw, SUBNET)?;
    if let Some(zone) = fields.raw("zone")?.filter(|raw| raw.get() != "null") {
        exact(zone.get(), &["uuid"])?;
    }
    nested_items(&fields, "secondarySubnets", |raw| {
        exact(raw, &["uuid", "name", "cidr"]).map(drop)
    })
}

fn acl_list_shape(raw: &str) -> Result<(), ()> {
    nested_items(&exact(raw, PAGED)?, "listData", |raw| {
        nested_items(&exact(raw, ACL)?, "aclPolicyRules", |raw| {
            exact(raw, ACL_RULE).map(drop)
        })
    })
}

fn decode<T: WireField>(raw: &str) -> Result<T, ()> {
    serde_json::from_str::<Field<T>>(raw)
        .map(|row| row.0)
        .map_err(|_| ())
}

fn identified<T>(row: T, id: &str, identity: impl FnOnce(&T) -> &str) -> Result<T, ()> {
    if identity(&row) != id {
        return Err(());
    }
    Ok(row)
}

fn vpc_output(raw: &str, id: &str) -> Result<Vpc, ()> {
    let mut row: Vpc = identified(decode(raw)?, id, |row: &Vpc| &row.uuid)?;
    row.zone.ip_ranges.get_or_insert_default();
    Ok(row)
}

fn subnet_output(raw: &str, id: &str) -> Result<Subnet, ()> {
    identified(Subnet::from(decode::<SubnetWire>(raw)?), id, |row| {
        &row.uuid
    })
}

fn nested<'a>(raw: &'a str, required: &[&str], keys: &[&str]) -> Result<(&'a str, Fields<'a>), ()> {
    let data = exact(raw, &["data"])?.raw("data")?.ok_or(())?.get();
    let fields = exact(data, keys)?;
    for key in required {
        if fields.raw(key)?.is_none_or(|value| value.get() == "null") {
            return Err(());
        }
    }
    Ok((data, fields))
}

pub(super) fn subnet_created(raw: &str) -> Result<Subnet, ()> {
    if raw == "null" {
        return Ok(Subnet::default());
    }
    subnet_shape(raw)?;
    decode::<SubnetWire>(raw).map(Subnet::from)
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl<'a> Write<'a> {
    async fn guard<T: Serialize + PublicOutput>(
        &mut self,
        ctx: &RequestContext,
        op: &'static str,
        read: Read,
        ids: &[&str],
        parse: impl FnOnce(&str) -> Result<T, ()>,
    ) -> Result<T, Error> {
        let raw = self.reads.request_in(ctx, read, ids, &[]).await;
        self.check_proofs()?;
        let raw = raw?;
        let row = parse(&raw).map_err(|()| self.invalid(op))?;
        self.safe(op, &row)?;
        crate::network::mutations::guard_operands(read.id, row, self.reads.evidence_mut())
    }

    pub(super) async fn vpc(&mut self, ctx: &RequestContext, id: &str) -> Result<Vpc, Error> {
        GET_VPC.path("VPCID", id)?;
        self.guard(ctx, "network.GetVPC", GET_VPC, &[id], |raw| {
            let fields = exact(raw, VPC)?;
            if let Some(zone) = fields.raw("zone")?.filter(|value| value.get() != "null") {
                exact(zone.get(), ZONE)?;
            }
            vpc_output(raw, id)
        })
        .await
    }

    pub(super) async fn renamed_vpc(&mut self, id: &str) -> Result<Vpc, Error> {
        self.rename_output("network.GetVPC", GET_VPC, &[id], |raw| vpc_output(raw, id))
            .await
    }

    pub(super) async fn renamed_subnet(&mut self, vpc: &str, id: &str) -> Result<Subnet, Error> {
        self.rename_output("network.GetSubnet", GET_SUBNET, &[vpc, id], |raw| {
            subnet_output(raw, id)
        })
        .await
    }

    async fn rename_output<T: WireField + Serialize + PublicOutput>(
        &mut self,
        op: &'static str,
        read: Read,
        ids: &[&str],
        parse: impl FnOnce(&str) -> Result<T, ()>,
    ) -> Result<T, Error> {
        let raw = self.reads.request_in(self.ctx, read, ids, &[]).await;
        self.check_proofs()?;
        let raw = raw?;
        let row = parse(&raw).map_err(|()| self.invalid(op))?;
        let row = self.reads.sanitize(read, row)?;
        self.safe(op, &row)?;
        crate::network::mutations::guard_operands(read.id, row, self.reads.evidence_mut())
    }

    pub(super) async fn dhcp(&mut self, id: &str) -> Result<DhcpOptions, Error> {
        GET_DHCP.path("DHCPOptionsID", id)?;
        self.guard(self.ctx, "network.GetDHCPOptions", GET_DHCP, &[id], |raw| {
            exact(raw, DHCP)?;
            decode(raw)
        })
        .await
    }

    pub(super) async fn subnet(
        &mut self,
        ctx: &RequestContext,
        vpc: &str,
        id: &str,
    ) -> Result<Subnet, Error> {
        GET_SUBNET.path("VPCID", vpc)?;
        self.guard(ctx, "network.GetSubnet", GET_SUBNET, &[vpc, id], |raw| {
            identified(subnet_created(raw)?, id, |row| &row.uuid)
        })
        .await
    }

    pub(super) async fn subnets(
        &mut self,
        ctx: &RequestContext,
        vpc: &str,
    ) -> Result<Vec<Subnet>, Error> {
        self.guard(ctx, "network.ListSubnetsByVPC", BY_VPC, &[vpc], |raw| {
            exact_items(raw, subnet_shape)?;
            let rows = decode::<Vec<SubnetWire>>(raw)?;
            let mut seen = HashSet::new();
            let rows: Vec<_> = rows.into_iter().map(Subnet::from).collect();
            if rows.iter().any(|row| {
                !PathIds::WithUnderscore.allows(&row.uuid) || !seen.insert(row.uuid.as_str())
            }) {
                return Err(());
            }
            Ok(rows)
        })
        .await
    }

    pub(super) async fn all_acls(
        &mut self,
        ctx: &RequestContext,
    ) -> Result<Vec<NetworkAcl>, Error> {
        let rows = self
            .reads
            .complete_in(
                ctx,
                LIST_ACLS,
                LIST_ACLS,
                PathIds::WithUnderscore,
                |row: &NetworkAcl| &row.uuid,
                acl_list_shape,
            )
            .await?;
        self.safe("network.ListNetworkACLs", &rows)?;
        crate::network::mutations::guard_operands(LIST_ACLS.id, rows, self.reads.evidence_mut())
    }

    pub(super) async fn acl(
        &mut self,
        ctx: &RequestContext,
        id: &str,
    ) -> Result<NetworkAcl, Error> {
        GET_ACL.path("NetworkACLID", id)?;
        let op = "network.GetNetworkACL";
        let result = self
            .guard(ctx, op, GET_ACL, &[id], |raw| {
                let required = [
                    "uuid",
                    "status",
                    "defaultAcl",
                    "interfaceNetworkUuid",
                    "subnetAssociationList",
                    "aclPolicyRules",
                ];
                let (data, fields) = nested(raw, &required, ACL)?;
                nested_items(&fields, "aclPolicyRules", |raw| {
                    exact(raw, ACL_RULE).map(drop)
                })?;
                let acl: NetworkAcl = decode(data)?;
                let mut seen = HashSet::new();
                if acl.uuid != id
                    || acl.status.is_empty()
                    || acl.vpc_id.is_empty()
                    || acl
                        .subnet_ids
                        .iter()
                        .any(|id| !PathIds::WithUnderscore.allows(id) || !seen.insert(id))
                {
                    return Err(());
                }
                Ok(acl)
            })
            .await;
        match result {
            Err(error) if is_5xx(&error) => {
                if self
                    .all_acls(ctx)
                    .await
                    .is_ok_and(|rows| rows.iter().all(|row| row.uuid != id && row.id != id))
                {
                    Err(self.local(
                        ErrorClass::NotFound,
                        &format!("vngcloud: resource not found: {op}: network ACL {id}"),
                    ))
                } else {
                    Err(self.screen(error))
                }
            }
            other => other,
        }
    }

    pub(super) async fn table(
        &mut self,
        ctx: &RequestContext,
        id: &str,
    ) -> Result<RouteTable, Error> {
        let (client, evidence) = (self.client, self.reads.evidence_mut());
        let project = client.inventory_project(ctx, &[id], evidence).await?;
        let response = client
            .read_route_inventory(
                ctx,
                client.vserver_inventory_base()?,
                ROUTE_TABLE,
                &[&project, id],
                &[],
                evidence,
            )
            .await?;
        let text = go_json(&response);
        ROUTE_TABLE.fields(&text, evidence)?;
        let parse = || {
            let (data, fields) = nested(&text, &["routes"], TABLE)?;
            nested_items(&fields, "routes", |raw| exact(raw, ROUTE).map(drop))?;
            let table: RouteTable = decode(data)?;
            if table.uuid != id || table.status.is_empty() {
                return Err(());
            }
            Ok(table)
        };
        let row = parse().map_err(|()| ROUTE_TABLE.incomplete(evidence))?;
        self.safe("network.GetRouteTable", &row)?;
        crate::network::mutations::guard_operands(ROUTE_TABLE.id, row, self.reads.evidence_mut())
    }

    pub(super) async fn interfaces(&mut self, subnet: &str) -> Result<usize, Error> {
        self.dependents(
            INTERFACES,
            &[
                "id",
                "uuid",
                "name",
                "fixedIp",
                "ipAddress",
                "mac",
                "networkId",
                "networkUuid",
                "subnetId",
                "subnetUuid",
                "serverId",
                "serverUuid",
                "status",
                "createdAt",
            ],
            |item: &ElasticNetworkInterface| item.subnet_id == subnet || item.subnet_uuid == subnet,
        )
        .await
    }

    pub(super) async fn virtual_ips(&mut self, subnet: &str) -> Result<usize, Error> {
        self.dependents(
            VIRTUAL_IPS,
            &[
                "id",
                "uuid",
                "name",
                "ipAddress",
                "networkId",
                "subnetId",
                "description",
                "subnetCIDR",
                "networkCIDR",
                "addressPairIps",
                "status",
                "createdAt",
                "networkName",
                "subnetName",
                "type",
                "mode",
                "zone",
            ],
            |item: &VirtualIpAddress| item.subnet_id == subnet,
        )
        .await
    }

    async fn dependents<T: WireField + Serialize + PublicOutput>(
        &mut self,
        read: InventoryRead,
        keys: &[&str],
        attached: impl Fn(&T) -> bool,
    ) -> Result<usize, Error> {
        let ctx = self.scoped(super::super::wire::walk_deadline());
        let (client, evidence) = (self.client, self.reads.evidence_mut());
        let project = client.inventory_project(&ctx, &[], evidence).await?;
        let base = client.vserver_inventory_base()?;
        let size = DEFAULT_PAGE_SIZE.to_string();
        let mut metadata = None;
        let mut found = 0;
        for page in DEFAULT_PAGE..=MAX_DEPENDENT_PAGES {
            let page_text = page.to_string();
            let query = [
                ("name", ""),
                ("page", page_text.as_str()),
                ("size", size.as_str()),
            ];
            let response = client
                .read_route_inventory(&ctx, base, read, &[&project], &query, evidence)
                .await?;
            let text = go_json(&response);
            let fields = read.fields(&text, evidence)?;
            let incomplete = || read.incomplete(evidence);
            let reported = |field| {
                fields
                    .required::<Field<Option<i64>>>(field)
                    .ok()
                    .and_then(|Field(value)| value)
                    .ok_or_else(incomplete)
            };
            let page_size = reported("pageSize")?;
            let pages = reported("totalPage")?;
            let total = reported("totalItem")?;
            let current = (page_size, pages, total);
            if reported("page")? != page
                || !(1..=DEFAULT_PAGE_SIZE).contains(&page_size)
                || !(0..=MAX_DEPENDENT_PAGES).contains(&pages)
                || !(0..=MAX_DEPENDENT_PAGES * DEFAULT_PAGE_SIZE).contains(&total)
                || (total > 0 && pages != 1 + (total - 1) / page_size)
                || (total == 0 && pages > 1)
                || *metadata.get_or_insert(current) != current
            {
                return Err(incomplete());
            }
            let raw = fields
                .raw("listData")
                .map_err(|()| read.invalid(evidence))?
                .ok_or_else(incomplete)?
                .get();
            let items: Vec<&RawValue> = serde_json::from_str(raw).map_err(|_| incomplete())?;
            let count = i64::try_from(items.len()).map_err(|_| incomplete())?;
            let expected = (total - (page - 1) * page_size).min(page_size);
            if count != expected {
                return Err(incomplete());
            }
            for item in items {
                exact(item.get(), keys).map_err(|()| read.invalid(evidence))?;
            }
            let rows: Vec<T> = decode(raw).map_err(|()| read.invalid(evidence))?;
            super::wire::screen_value(&rows, evidence).map_err(|()| read.invalid(evidence))?;
            let rows = crate::network::mutations::guard_operands(read.id, rows, evidence)?;
            found += rows.iter().filter(|row| attached(row)).count();
            if page >= pages {
                return Ok(found);
            }
        }
        Err(read.incomplete(evidence))
    }

    pub(super) async fn servers(&mut self, id: &str) -> Result<usize, Error> {
        let client: &Client = self.client;
        let rows = client
            .guard_servers_by_subnet(self.ctx, id, self.reads.evidence_mut())
            .await?;
        Ok(rows.items.len())
    }

    pub(super) async fn active<T>(
        &mut self,
        op: &'static str,
        resource: &str,
        mut read: impl for<'b> AsyncFnMut(&'b mut Write<'a>, &'b RequestContext) -> Result<T, Error>,
        status: fn(&T) -> &str,
    ) -> Result<T, Error> {
        let ctx = self.scoped(BOUND);
        guarded_poll!(self.reads.evidence_mut(), {
            if self.ended(&ctx) {
                break;
            }
            let row = read(self, &ctx).await;
            self.check_proofs()?;
            if self.ended(&ctx) {
                break;
            }
            let row = row?;
            if status(&row) == "ACTIVE" {
                return Ok(row);
            }
            self.tick(&ctx, INTERVAL).await;
        });
        if let Some(end) = self.ctx.check(self.client.session.clock()) {
            return Err(end.into());
        }
        Err(self.guard_error(
            NetworkWriteError::Busy,
            &format!("{op}: {resource} is not ACTIVE within 1m0s; nothing sent"),
        ))
    }

    pub(super) async fn settle<T>(
        &mut self,
        bound: Duration,
        interval: Duration,
        timeout: &str,
        mut step: impl for<'b> AsyncFnMut(
            &'b mut Write<'a>,
            &'b RequestContext,
        ) -> Result<Option<T>, Error>,
    ) -> Result<T, Error> {
        let ctx = self.scoped(bound);
        guarded_poll!(self.reads.evidence_mut(), {
            if self.ended(&ctx) {
                break;
            }
            let result = step(self, &ctx).await;
            self.check_proofs()?;
            if self.ended(&ctx) {
                break;
            }
            if let Some(result) = result? {
                return Ok(result);
            }
            self.tick(&ctx, interval).await;
        });
        let error = if let Some(end) = self.ctx.check(self.client.session.clock()) {
            self.settled_error(timeout.split(';').next().unwrap_or(timeout), end.into())
        } else {
            self.local(
                ErrorClass::NotSettled,
                &format!("vngcloud: write outcome not confirmed: {timeout}"),
            )
        };
        Err(error)
    }

    pub(super) fn status_error(&self, op: &'static str, resource: &str) -> Error {
        self.local(
            ErrorClass::Failed,
            &format!("vngcloud: write failed: {op}: {resource} is ERROR"),
        )
    }
}
