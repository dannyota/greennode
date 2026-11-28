use crate::model::wire::model;
use crate::{Failure, List, RequestContext};

use super::super::{Client, failure};
use super::models::*;
use super::wire::{Read, Reads, Style};

pub(super) const GET_VPC: Read = Read::new("network.GetVPC", "v2/{}/networks/{}", Style::Plain);
const LIST_VPCS: Read = Read::new("network.ListVPCs", "v2/{}/networks", Style::Plain);
pub(super) const GET_SUBNET: Read = Read::new(
    "network.GetSubnet",
    "v2/{}/networks/{}/subnets/{}",
    Style::Plain,
);
pub(super) const BY_VPC: Read = Read::new(
    "network.ListSubnetsByVPC",
    "v2/{}/networks/{}/subnets",
    Style::Plain,
);

model! {
    @ true;
    pub(super) struct SubnetWire {
        uuid: String => "uuid", "UUID";
        status: String => "status", "Status";
        cidr: String => "cidr", "CIDR";
        network_uuid: String => "networkUuid", "NetworkUUID";
        route_table_uuid: String => "routeTableUuid", "RouteTableUUID";
        name: String => "name", "Name";
        interface_acl_policy_uuid: String => "interfaceAclPolicyUuid", "InterfaceACLPolicyUUID";
        interface_acl_policy_name: String => "interfaceAclPolicyName", "InterfaceACLPolicyName";
        secondary_subnets: Vec<SubnetSecondarySubnet> => "secondarySubnets", "SecondarySubnets";
        zone: SubnetZone => "zone", "Zone";
    }
}

model! {
    @ true;
    pub(super) struct SubnetZone {
        uuid: String => "uuid", "UUID";
    }
}

impl From<SubnetWire> for Subnet {
    fn from(row: SubnetWire) -> Self {
        Self {
            uuid: row.uuid,
            name: row.name,
            network_id: row.network_uuid,
            cidr: row.cidr,
            status: row.status,
            interface_acl_policy_id: row.interface_acl_policy_uuid,
            interface_acl_policy_name: row.interface_acl_policy_name,
            route_table_id: row.route_table_uuid,
            zone_id: row.zone.uuid,
            zone: super::models::Zone {
                ip_ranges: Some(Vec::new()),
                ..Default::default()
            },
            secondary_subnets: row.secondary_subnets,
            ..Self::default()
        }
    }
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Client {
    /// Reads a VPC by ID.
    pub async fn get_vpc(
        &self,
        ctx: &RequestContext,
        input: GetVpcInput,
    ) -> Result<GetVpcOutput, Failure<GetVpcOutput>> {
        crate::wire::product::exit::finish!(async {
            GET_VPC.path("VPCID", &input.vpc_id).map_err(failure)?;
            let mut reads = Reads::new(self, ctx).map_err(failure)?;
            let mut row: Vpc = reads
                .get(GET_VPC, &[&input.vpc_id])
                .await
                .map_err(failure)?;
            row.zone.ip_ranges.get_or_insert_default();
            let vpc = reads.sanitize(GET_VPC, row).map_err(failure)?;
            Ok(GetVpcOutput { vpc })
        })
    }

    /// Reads a page of VPCs, with an optional name filter.
    pub async fn list_vpcs(
        &self,
        ctx: &RequestContext,
        input: ListVpcsInput,
    ) -> Result<ListVpcsOutput, Failure<ListVpcsOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut reads = Reads::new(self, ctx).map_err(failure)?;
            let mut page: crate::PagedList<Vpc> = reads
                .page(LIST_VPCS, &input.name, input.page, input.size)
                .await
                .map_err(failure)?;
            for vpc in &mut page.items {
                vpc.zone.ip_ranges.get_or_insert_default();
            }
            reads.sanitize_page(LIST_VPCS, page).map_err(failure)
        })
    }

    /// Reads a subnet in the specified VPC.
    pub async fn get_subnet(
        &self,
        ctx: &RequestContext,
        input: GetSubnetInput,
    ) -> Result<GetSubnetOutput, Failure<GetSubnetOutput>> {
        crate::wire::product::exit::finish!(async {
            if !input.vpc_id.is_empty() && input.subnet_id.is_empty() {
                GET_SUBNET
                    .path("SubnetID", &input.subnet_id)
                    .map_err(failure)?;
            }
            GET_SUBNET.path("VPCID", &input.vpc_id).map_err(failure)?;
            GET_SUBNET
                .path("SubnetID", &input.subnet_id)
                .map_err(failure)?;
            let mut reads = Reads::new(self, ctx).map_err(failure)?;
            let row: SubnetWire = reads
                .get(GET_SUBNET, &[&input.vpc_id, &input.subnet_id])
                .await
                .map_err(failure)?;
            let subnet = reads
                .sanitize(GET_SUBNET, Subnet::from(row))
                .map_err(failure)?;
            Ok(GetSubnetOutput { subnet })
        })
    }

    /// Lists the subnets in the specified VPC.
    pub async fn list_subnets_by_vpc(
        &self,
        ctx: &RequestContext,
        input: ListSubnetsByVpcInput,
    ) -> Result<ListSubnetsByVpcOutput, Failure<ListSubnetsByVpcOutput>> {
        crate::wire::product::exit::finish!(async {
            BY_VPC.path("VPCID", &input.vpc_id).map_err(failure)?;
            let mut reads = Reads::new(self, ctx).map_err(failure)?;
            let rows: Vec<SubnetWire> =
                reads.get(BY_VPC, &[&input.vpc_id]).await.map_err(failure)?;
            let items = reads
                .sanitize(
                    BY_VPC,
                    rows.into_iter().map(Subnet::from).collect::<Vec<_>>(),
                )
                .map_err(failure)?;
            Ok(List { items })
        })
    }

    /// Lists subnets across the VPCs returned by the default inventory page.
    pub async fn list_subnets(
        &self,
        ctx: &RequestContext,
        _: ListSubnetsInput,
    ) -> Result<ListSubnetsOutput, Failure<ListSubnetsOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut reads = Reads::new(self, ctx).map_err(failure)?;
            let vpcs = reads
                .page::<Vpc>(LIST_VPCS, "", 0, 0)
                .await
                .map_err(failure)?;
            if vpcs.items.len() > crate::DEFAULT_PAGE_SIZE as usize {
                return Err(failure(reads.invalid(LIST_VPCS)));
            }
            let mut items = Vec::new();
            let mut retained = 0;
            for vpc in vpcs.items {
                BY_VPC.path("VPCID", &vpc.uuid).map_err(failure)?;
                let rows: Vec<SubnetWire> =
                    reads.get(BY_VPC, &[&vpc.uuid]).await.map_err(failure)?;
                reads
                    .append_bounded(
                        BY_VPC,
                        &mut retained,
                        &mut items,
                        rows.into_iter().map(Subnet::from).collect(),
                    )
                    .map_err(failure)?;
            }
            let items = reads.sanitize(BY_VPC, items).map_err(failure)?;
            Ok(List { items })
        })
    }
}
