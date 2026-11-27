use crate::model::wire::model;
use crate::{Failure, PagedList, RequestContext};

use super::route_inventory::{InventoryRead, paged_input};
use super::{Client, failure};

const WAN_IPS: InventoryRead = InventoryRead::standard("network.ListWANIPs", "v2/{}/wanIps");
const INTERFACES: InventoryRead = InventoryRead::standard(
    "network.ListNetworkInterfaces",
    "v2/{}/network-interfaces-elastic",
);
const PEERINGS: InventoryRead = InventoryRead::standard("network.ListPeerings", "v2/{}/peering");
const INTERCONNECTS: InventoryRead =
    InventoryRead::standard("network.ListInterconnects", "v2/{}/interconnects");

paged_input!(
    /// Selects a page of WAN IPs.
    ListWanIpsInput
);

/// One page of project WAN IPs.
pub type ListWanIpsOutput = PagedList<WanIp>;

paged_input!(
    /// Selects a page of elastic network interfaces.
    ListNetworkInterfacesInput
);

/// One page of elastic network interfaces.
pub type ListNetworkInterfacesOutput = PagedList<ElasticNetworkInterface>;

paged_input!(
    /// Selects a page of VPC peerings.
    ListPeeringsInput
);

/// One page of VPC peerings.
pub type ListPeeringsOutput = PagedList<Peering>;

paged_input!(
    /// Selects a page of project interconnects.
    ListInterconnectsInput
);

/// One page of project interconnects.
pub type ListInterconnectsOutput = PagedList<Interconnect>;

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl Client {
    /// Lists one page of WAN IPs in the selected project.
    pub async fn list_wan_ips(
        &self,
        ctx: &RequestContext,
        input: ListWanIpsInput,
    ) -> Result<ListWanIpsOutput, Failure<ListWanIpsOutput>> {
        crate::wire::product::exit::finish!(async {
            self.list_route_inventory(ctx, WAN_IPS, (&input.name, input.page, input.size))
                .await
                .map_err(failure)
        })
    }

    /// Lists one page of elastic network interfaces in the selected project.
    pub async fn list_network_interfaces(
        &self,
        ctx: &RequestContext,
        input: ListNetworkInterfacesInput,
    ) -> Result<ListNetworkInterfacesOutput, Failure<ListNetworkInterfacesOutput>> {
        crate::wire::product::exit::finish!(async {
            self.list_route_inventory(ctx, INTERFACES, (&input.name, input.page, input.size))
                .await
                .map_err(failure)
        })
    }

    /// Lists one page of VPC peerings in the selected project.
    pub async fn list_peerings(
        &self,
        ctx: &RequestContext,
        input: ListPeeringsInput,
    ) -> Result<ListPeeringsOutput, Failure<ListPeeringsOutput>> {
        crate::wire::product::exit::finish!(async {
            self.list_route_inventory(ctx, PEERINGS, (&input.name, input.page, input.size))
                .await
                .map_err(failure)
        })
    }

    /// Lists one page of interconnects in the selected project.
    pub async fn list_interconnects(
        &self,
        ctx: &RequestContext,
        input: ListInterconnectsInput,
    ) -> Result<ListInterconnectsOutput, Failure<ListInterconnectsOutput>> {
        crate::wire::product::exit::finish!(async {
            self.list_route_inventory(ctx, INTERCONNECTS, (&input.name, input.page, input.size))
                .await
                .map_err(failure)
        })
    }
}

model! {
    @ true;
    /// A public IP address in the selected project.
    pub struct WanIp {
        /// Provider resource identifier.
        uuid: String => "uuid", "UUID";
        /// Alternate provider identifier.
        id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// IP address of the resource.
        ip_address: String => "ipAddress", "IPAddress";
        /// Floating IP assigned to the interface.
        floating_ip: String => "floatingIp", "FloatingIP";
        /// Identifier of the attached network interface.
        network_interface_id: String => "networkInterfaceId", "NetworkInterfaceID";
        /// Attached server identifier.
        server_id: String => "serverId", "ServerID";
        /// Provider-reported state.
        status: String => "status", "Status";
        /// Provider resource type.
        kind: String => "type", "Type";
        /// Creation time as supplied by the service.
        created_at: String => "createdAt", "CreatedAt";
    }
}

model! {
    @ true;
    /// An elastic network interface in the selected project.
    pub struct ElasticNetworkInterface {
        /// Provider resource identifier.
        uuid: String => "uuid", "UUID";
        /// Alternate provider identifier.
        id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// Fixed IP assigned to the interface.
        fixed_ip: String => "fixedIp", "FixedIP";
        /// IP address of the resource.
        ip_address: String => "ipAddress", "IPAddress";
        /// Interface MAC address.
        mac: String => "mac", "MAC";
        /// Network identifier.
        network_id: String => "networkId", "NetworkID";
        /// Network UUID.
        network_uuid: String => "networkUuid", "NetworkUUID";
        /// Subnet identifier.
        subnet_id: String => "subnetId", "SubnetID";
        /// Subnet UUID.
        subnet_uuid: String => "subnetUuid", "SubnetUUID";
        /// Attached server identifier.
        server_id: String => "serverId", "ServerID";
        /// Attached server UUID.
        server_uuid: String => "serverUuid", "ServerUUID";
        /// Provider-reported state.
        status: String => "status", "Status";
        /// Creation time as supplied by the service.
        created_at: String => "createdAt", "CreatedAt";
    }
}

model! {
    @ true;
    /// A connection between two VPCs.
    pub struct Peering {
        /// Provider resource identifier.
        uuid: String => "uuid", "UUID";
        /// Display name.
        name: String => "name", "Name";
        /// Provider-reported state.
        status: String => "status", "Status";
        /// Source VPC identifier.
        from_vpc_id: String => "fromVpcId", "FromVPCID";
        /// Source VPC UUID.
        from_vpc_uuid: String => "fromVpcUuid", "FromVPCUUID";
        /// Source VPC address range.
        from_cidr: String => "fromCidr", "FromCIDR";
        /// Destination VPC identifier.
        end_vpc_id: String => "endVpcId", "EndVPCID";
        /// Destination VPC UUID.
        end_vpc_uuid: String => "endVpcUuid", "EndVPCUUID";
        /// Destination VPC address range.
        end_cidr: String => "endCidr", "EndCIDR";
        /// Creation time as supplied by the service.
        created_at: String => "createdAt", "CreatedAt";
    }
}

model! {
    @ true;
    /// A project interconnect and its gateway details.
    pub struct Interconnect {
        /// Provider resource identifier.
        uuid: String => "uuid", "UUID";
        /// Alternate provider identifier.
        id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// Provider-reported state.
        status: String => "status", "Status";
        /// Resource description.
        description: String => "description", "Description";
        /// Interconnect type identifier.
        type_id: String => "typeId", "TypeID";
        /// Interconnect type name.
        type_name: String => "typeName", "TypeName";
        /// Package identifier.
        package_id: String => "packageId", "PackageID";
        /// Creation time as supplied by the service.
        created_at: String => "createdAt", "CreatedAt";
        /// Project identifier.
        project_id: String => "projectId", "ProjectID";
        /// Circuit identifier in the provider-supplied JSON shape.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        circuit_id: serde_json::Value => "circuitId", "CircuitID";
        /// Whether the second gateway is enabled.
        enable_gw2: bool => "enableGw2", "EnableGW2";
        /// First local gateway address.
        gw01_ip: String => "gw01Ip", "GW01IP";
        /// Second local gateway address.
        gw02_ip: String => "gw02Ip", "GW02IP";
        /// Local virtual gateway address.
        gw_vip: String => "gwVip", "GWVIP";
        /// First remote gateway address.
        remote_gw01_ip: String => "remoteGw01Ip", "RemoteGW01IP";
        /// Second remote gateway address.
        remote_gw02_ip: String => "remoteGw02Ip", "RemoteGW02IP";
    }
}
