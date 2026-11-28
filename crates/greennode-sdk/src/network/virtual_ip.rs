use std::collections::HashSet;

use serde::Serialize;

use crate::model::wire::{WireField, decode_lenient, go_json, model};
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::{Error, Failure, List, PagedList, RequestContext, model};

use super::route_inventory::{
    CAP, InventoryRead, field, ids_reflect, paged_input, required_id_input,
};
use super::{
    Client, Zone, failure,
    mutations::{retain, strict_field},
};

const VIRTUAL_IP: InventoryRead =
    InventoryRead::standard("network.GetVirtualIPAddress", "v2/{}/virtualIpAddress/{}");
pub(super) const VIRTUAL_IPS: InventoryRead =
    InventoryRead::standard("network.ListVirtualIPAddresses", "v2/{}/virtualIpAddress");
const VIP_PAIRS: InventoryRead = InventoryRead::standard(
    "network.ListAddressPairsByVirtualIPAddress",
    "v2/{}/virtualIpAddress/{}/addressPairs",
);
const SUBNET_PAIRS: InventoryRead = InventoryRead::standard(
    "network.ListAddressPairsByVirtualSubnet",
    "v2/{}/virtual-subnets/{}/addressPairs",
);
const ALL_PAIRS: InventoryRead =
    InventoryRead::standard("network.ListAllVirtualIPAddressAddressPairs", "");

required_id_input!(
    /// Selects a virtual IP address to read.
    GetVirtualIpAddressInput,
    virtual_ip_address_id
);

/// The selected virtual IP address.
#[derive(Clone, PartialEq, Serialize)]
pub struct GetVirtualIpAddressOutput {
    /// The virtual IP and its reported settings.
    #[serde(rename = "VirtualIPAddress")]
    pub virtual_ip_address: VirtualIpAddress,
}

crate::wire::product::output::public_fields!(GetVirtualIpAddressOutput { virtual_ip_address });

paged_input!(
    /// Selects a page of virtual IP addresses.
    ListVirtualIpAddressesInput
);

/// One page of virtual IP addresses.
pub type ListVirtualIpAddressesOutput = PagedList<VirtualIpAddress>;

required_id_input!(
    /// Selects the virtual IP whose address pairs to list.
    ListAddressPairsByVirtualIpAddressInput,
    virtual_ip_address_id
);

/// Address pairs for the selected virtual IP, in provider order.
pub type ListAddressPairsByVirtualIpAddressOutput = List<AddressPair>;

required_id_input!(
    /// Selects the virtual subnet whose address pairs to list.
    ListAddressPairsByVirtualSubnetInput,
    virtual_subnet_id
);

/// Address pairs for the selected virtual subnet, in provider order.
pub type ListAddressPairsByVirtualSubnetOutput = List<AddressPair>;

/// Lists the address pairs for the default page of virtual IPs.
#[derive(Clone, Default)]
pub struct ListAllVirtualIpAddressAddressPairsInput {}

crate::error::debug_record!(ListAllVirtualIpAddressAddressPairsInput {});

/// Address pairs from the default page of virtual IPs, in provider order.
pub type ListAllVirtualIpAddressAddressPairsOutput = List<AddressPair>;

#[derive(Clone, Copy)]
enum Parent<'a> {
    VirtualIp(&'a str),
    VirtualSubnet(&'a str),
}

impl<'a> Parent<'a> {
    fn route(self) -> (InventoryRead, &'static str, &'a str, PathIds) {
        match self {
            Self::VirtualIp(id) => (VIP_PAIRS, "VirtualIPAddressID", id, PathIds::Strict),
            Self::VirtualSubnet(id) => {
                (SUBNET_PAIRS, "VirtualSubnetID", id, PathIds::WithUnderscore)
            }
        }
    }

    fn assign(self, row: &mut AddressPair) {
        match self {
            Self::VirtualIp(id) => row.virtual_ip_address_id = id.to_owned(),
            Self::VirtualSubnet(id) => row.virtual_subnet_id = id.to_owned(),
        }
    }
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl Client {
    /// Reads a virtual IP address in the selected project.
    pub async fn get_virtual_ip_address(
        &self,
        ctx: &RequestContext,
        input: GetVirtualIpAddressInput,
    ) -> Result<GetVirtualIpAddressOutput, Failure<GetVirtualIpAddressOutput>> {
        crate::wire::product::exit::finish!(async {
            let id = &input.virtual_ip_address_id;
            let mut evidence = self.request_evidence(ctx).map_err(failure)?;
            VIRTUAL_IP
                .check_id("VirtualIPAddressID", id, PathIds::Strict, &evidence)
                .map_err(failure)?;
            let virtual_ip_address = self
                .virtual_ip_detail(ctx, id, false, &mut evidence)
                .await
                .map_err(failure)?;
            Ok(GetVirtualIpAddressOutput { virtual_ip_address })
        })
    }

    pub(super) async fn virtual_ip_detail(
        &self,
        ctx: &RequestContext,
        id: &str,
        guarded: bool,
        evidence: &mut Evidence,
    ) -> Result<VirtualIpAddress, Error> {
        let _guard_reads = guarded.then(|| evidence.guard_reads());
        let project = self.inventory_project(ctx, &[id], evidence).await?;
        let base = self.vserver_inventory_base()?;
        let raw = self
            .read_route_inventory(ctx, base, VIRTUAL_IP, &[&project, id], &[], evidence)
            .await?;
        let text = go_json(&raw);
        let envelope = VIRTUAL_IP.fields(&text, evidence)?;
        let invalid = |()| {
            if guarded {
                VIRTUAL_IP.incomplete(evidence)
            } else {
                VIRTUAL_IP.invalid(evidence)
            }
        };
        let row: VirtualIpAddress = if guarded {
            strict_field(&envelope, "data")
        } else {
            decode_lenient(|| field(&envelope, "data"))
        }
        .map_err(invalid)?;
        if guarded && row.uuid != id {
            return Err(invalid(()));
        }
        retain(row, evidence).ok_or_else(|| invalid(()))
    }

    pub(super) async fn guard_vip_pairs(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<Vec<AddressPair>, Error> {
        let _guard_reads = evidence.guard_reads();
        let project = self.inventory_project(ctx, &[id], evidence).await?;
        let (items, _) = self
            .read_address_pairs(ctx, &project, Parent::VirtualIp(id), true, evidence)
            .await?;
        retain(items, evidence).ok_or_else(|| VIP_PAIRS.invalid(evidence))
    }

    /// Lists one page of virtual IP addresses for the selected project.
    pub async fn list_virtual_ip_addresses(
        &self,
        ctx: &RequestContext,
        input: ListVirtualIpAddressesInput,
    ) -> Result<ListVirtualIpAddressesOutput, Failure<ListVirtualIpAddressesOutput>> {
        crate::wire::product::exit::finish!(async {
            self.list_route_inventory(ctx, VIRTUAL_IPS, (&input.name, input.page, input.size))
                .await
                .map_err(failure)
        })
    }

    /// Lists address pairs attached to one virtual IP.
    pub async fn list_address_pairs_by_virtual_ip_address(
        &self,
        ctx: &RequestContext,
        input: ListAddressPairsByVirtualIpAddressInput,
    ) -> Result<
        ListAddressPairsByVirtualIpAddressOutput,
        Failure<ListAddressPairsByVirtualIpAddressOutput>,
    > {
        crate::wire::product::exit::finish!(async {
            self.list_address_pairs(ctx, Parent::VirtualIp(&input.virtual_ip_address_id))
                .await
                .map_err(failure)
        })
    }

    /// Lists address pairs attached to one virtual subnet.
    pub async fn list_address_pairs_by_virtual_subnet(
        &self,
        ctx: &RequestContext,
        input: ListAddressPairsByVirtualSubnetInput,
    ) -> Result<ListAddressPairsByVirtualSubnetOutput, Failure<ListAddressPairsByVirtualSubnetOutput>>
    {
        crate::wire::product::exit::finish!(async {
            self.list_address_pairs(ctx, Parent::VirtualSubnet(&input.virtual_subnet_id))
                .await
                .map_err(failure)
        })
    }

    /// Reads the address pairs for each virtual IP on the default parent page.
    pub async fn list_all_virtual_ip_address_address_pairs(
        &self,
        ctx: &RequestContext,
        _input: ListAllVirtualIpAddressAddressPairsInput,
    ) -> Result<
        ListAllVirtualIpAddressAddressPairsOutput,
        Failure<ListAllVirtualIpAddressAddressPairsOutput>,
    > {
        crate::wire::product::exit::finish!(async {
            let mut evidence = self.request_evidence(ctx).map_err(failure)?;
            let project = self
                .inventory_project(ctx, &[], &mut evidence)
                .await
                .map_err(failure)?;
            let (parents, mut bytes) = self
                .inventory_page::<VirtualIpAddress>(
                    ctx,
                    VIRTUAL_IPS,
                    &project,
                    ("", 0, 0),
                    &mut evidence,
                )
                .await
                .map_err(failure)?;
            let mut seen = HashSet::new();
            if parents.items.len() as i64 > model::DEFAULT_PAGE_SIZE
                || parents.items.iter().any(|item| {
                    !PathIds::Strict.allows(&item.uuid)
                        || evidence.id_reflects(&item.uuid)
                        || !seen.insert(item.uuid.as_str())
                })
            {
                return Err(failure(ALL_PAIRS.incomplete(&evidence)));
            }
            let mut items = Vec::new();
            for virtual_ip in parents.items {
                let id = &virtual_ip.uuid;
                if evidence.id_reflects(id) {
                    return Err(failure(ALL_PAIRS.incomplete(&evidence)));
                }
                let (pairs, size) = self
                    .read_address_pairs(ctx, &project, Parent::VirtualIp(id), false, &mut evidence)
                    .await
                    .map_err(failure)?;
                bytes = bytes.saturating_add(size);
                if bytes > CAP {
                    return Err(failure(ALL_PAIRS.incomplete(&evidence)));
                }
                items.extend(pairs);
            }
            if ids_reflect(&items, &evidence) || !items.redact(&evidence) {
                return Err(failure(ALL_PAIRS.invalid(&evidence)));
            }
            Ok(List { items })
        })
    }

    async fn list_address_pairs(
        &self,
        ctx: &RequestContext,
        parent: Parent<'_>,
    ) -> Result<List<AddressPair>, Error> {
        let (read, field, id, charset) = parent.route();
        let mut evidence = self.request_evidence(ctx)?;
        read.check_id(field, id, charset, &evidence)?;
        let project = self.inventory_project(ctx, &[id], &mut evidence).await?;
        let (mut items, _) = self
            .read_address_pairs(ctx, &project, parent, false, &mut evidence)
            .await?;
        if ids_reflect(&items, &evidence) || !items.redact(&evidence) {
            return Err(read.invalid(&evidence));
        }
        Ok(List { items })
    }

    async fn read_address_pairs(
        &self,
        ctx: &RequestContext,
        project: &str,
        parent: Parent<'_>,
        guarded: bool,
        evidence: &mut Evidence,
    ) -> Result<(Vec<AddressPair>, usize), Error> {
        let (read, _, id, _) = parent.route();
        let raw = self
            .read_route_inventory(
                ctx,
                self.vserver_inventory_base()?,
                read,
                &[project, id],
                &[],
                evidence,
            )
            .await?;
        let text = go_json(&raw);
        let envelope = read.fields(&text, evidence)?;
        let mut items: Vec<AddressPair> = if guarded {
            strict_field(&envelope, "data")
        } else {
            decode_lenient(|| field(&envelope, "data"))
        }
        .map_err(|()| {
            if guarded {
                read.incomplete(evidence)
            } else {
                read.invalid(evidence)
            }
        })?;
        let bytes = raw
            .len()
            .saturating_add(id.len().saturating_mul(items.len()));
        if bytes > CAP {
            return Err(read.incomplete(evidence));
        }
        for item in &mut items {
            parent.assign(item);
        }
        Ok((items, bytes))
    }
}

model! {
    /// A virtual IP address and its network placement.
    pub struct VirtualIpAddress {
        /// Provider resource identifier.
        uuid: String => "uuid", "UUID";
        /// Alternate provider identifier.
        id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// Virtual IP address.
        endpoint_address: String => "ipAddress", "EndpointAddress";
        /// VPC identifier.
        vpc_id: String => "networkId", "VPCID";
        /// Subnet identifier.
        subnet_id: String => "subnetId", "SubnetID";
        /// Resource description.
        description: String => "description", "Description";
        /// Subnet address range.
        subnet_cidr: String => "subnetCIDR", "SubnetCIDR";
        /// VPC address range.
        vpc_cidr: String => "networkCIDR", "VPCCIDR";
        /// Interface IP addresses paired with the virtual IP.
        address_pair_ips: Vec<String> => "addressPairIps", "AddressPairIPs";
        /// Provider-reported state.
        status: String => "status", "Status";
        /// Creation time as supplied by the service.
        created_at: String => "createdAt", "CreatedAt";
        /// Name of the containing network.
        network_name: String => "networkName", "NetworkName";
        /// Name of the containing subnet.
        subnet_name: String => "subnetName", "SubnetName";
        /// Provider resource type.
        kind: String => "type", "Type";
        /// Provider-reported mode.
        mode: String => "mode", "Mode";
        /// Reported network zone.
        zone: Zone => "zone", "Zone";
    }
}

model! {
    /// A virtual IP or subnet assignment to a network interface.
    pub struct AddressPair {
        /// Provider resource identifier.
        uuid: String => "uuid", "UUID";
        /// Virtual IP identifier.
        virtual_ip_address_id: String => "virtualIpAddressId", "VirtualIPAddressID";
        /// Virtual subnet identifier.
        virtual_subnet_id: String => "virtualSubnetId", "VirtualSubnetID";
        /// Address of the attached network interface.
        network_interface_ip: String => "networkInterfaceIp", "NetworkInterfaceIP";
        /// Identifier of the attached network interface.
        network_interface_id: String => "networkInterfaceId", "NetworkInterfaceID";
        /// Address range in CIDR notation.
        cidr: String => "cidr", "CIDR";
        /// Creation time as supplied by the service.
        created_at: String => "createdAt", "CreatedAt";
    }
}
