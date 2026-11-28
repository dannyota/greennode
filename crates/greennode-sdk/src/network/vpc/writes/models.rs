use serde::Serialize;

use crate::network::{DhcpOptions, NetworkAcl, RouteTable, Subnet, Vpc};

/// Name, address range and settlement preference for a new VPC.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CreateVpcInput {
    /// Resource display name.
    pub name: String,
    /// Address range in CIDR notation.
    pub cidr: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl CreateVpcInput {
    /// Sets the required operation inputs.
    pub fn new(name: impl Into<String>, cidr: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            cidr: cidr.into(),
            no_wait: false,
        }
    }
}

/// The created VPC or the last observed state.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CreateVpcOutput {
    /// The VPC returned by the operation.
    #[serde(rename = "VPC")]
    pub vpc: Vpc,
}

/// Selects a VPC and its replacement name.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct UpdateVpcInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
    /// Resource display name.
    pub name: String,
}

impl UpdateVpcInput {
    /// Sets the required operation inputs.
    pub fn new(vpc_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            vpc_id: vpc_id.into(),
            name: name.into(),
        }
    }
}

/// The VPC after a rename, or submitted fields if the read fails.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UpdateVpcOutput {
    /// The VPC returned by the operation.
    #[serde(rename = "VPC")]
    pub vpc: Vpc,
}

/// Selects a VPC and whether to wait for its removal.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DeleteVpcInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl DeleteVpcInput {
    /// Sets the required operation inputs.
    pub fn new(vpc_id: impl Into<String>) -> Self {
        Self {
            vpc_id: vpc_id.into(),
            no_wait: false,
        }
    }
}

/// An accepted VPC deletion has no response fields.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DeleteVpcOutput {}

/// Selects the VPC whose private DNS should be enabled.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct EnableVpcPrivateDnsInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl EnableVpcPrivateDnsInput {
    /// Sets the required operation inputs.
    pub fn new(vpc_id: impl Into<String>) -> Self {
        Self {
            vpc_id: vpc_id.into(),
            no_wait: false,
        }
    }
}

/// Private DNS state and whether an enable was needed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EnableVpcPrivateDnsOutput {
    /// The VPC returned by the operation.
    #[serde(rename = "VPC")]
    pub vpc: Vpc,
    /// Whether the operation changed the resource.
    #[serde(rename = "Changed")]
    pub changed: bool,
}

/// Settings for a new DHCP options set.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CreateDhcpOptionsInput {
    /// Resource display name.
    pub name: String,
    /// IPv4 addresses of the DNS servers.
    pub dns_servers: Vec<String>,
    /// Optional maximum transmission unit.
    pub mtu: Option<i64>,
}

impl CreateDhcpOptionsInput {
    /// Sets the required operation inputs.
    pub fn new(name: impl Into<String>, dns_servers: Vec<String>) -> Self {
        Self {
            name: name.into(),
            dns_servers,
            mtu: None,
        }
    }
}

/// The DHCP options set returned by the service.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CreateDhcpOptionsOutput {
    /// The DHCP options set returned by the operation.
    #[serde(rename = "DHCPOptions")]
    pub dhcp_options: DhcpOptions,
}

/// Selects the unattached DHCP options set to delete.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DeleteDhcpOptionsInput {
    /// Identifier of the DHCP options set.
    pub dhcp_options_id: String,
}

impl DeleteDhcpOptionsInput {
    /// Sets the required operation inputs.
    pub fn new(dhcp_options_id: impl Into<String>) -> Self {
        Self {
            dhcp_options_id: dhcp_options_id.into(),
        }
    }
}

/// An accepted DHCP options deletion has no response fields.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DeleteDhcpOptionsOutput {}

/// Selects a VPC and a DHCP options set to attach.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SetVpcDhcpOptionsInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
    /// Identifier of the DHCP options set.
    pub dhcp_options_id: String,
}

impl SetVpcDhcpOptionsInput {
    /// Sets the required operation inputs.
    pub fn new(vpc_id: impl Into<String>, dhcp_options_id: impl Into<String>) -> Self {
        Self {
            vpc_id: vpc_id.into(),
            dhcp_options_id: dhcp_options_id.into(),
        }
    }
}

/// The VPC and whether its DHCP options attachment changed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SetVpcDhcpOptionsOutput {
    /// The VPC returned by the operation.
    #[serde(rename = "VPC")]
    pub vpc: Vpc,
    /// Whether the operation changed the resource.
    #[serde(rename = "Changed")]
    pub changed: bool,
}

/// Selects the VPC whose DHCP options set should be detached.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ClearVpcDhcpOptionsInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
}

impl ClearVpcDhcpOptionsInput {
    /// Sets the required operation inputs.
    pub fn new(vpc_id: impl Into<String>) -> Self {
        Self {
            vpc_id: vpc_id.into(),
        }
    }
}

/// The VPC and whether its DHCP options attachment changed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ClearVpcDhcpOptionsOutput {
    /// The VPC returned by the operation.
    #[serde(rename = "VPC")]
    pub vpc: Vpc,
    /// Whether the operation changed the resource.
    #[serde(rename = "Changed")]
    pub changed: bool,
}

/// Parent VPC, zone, name and address range for a new subnet.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CreateSubnetInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
    /// Availability zone identifier.
    pub zone_id: String,
    /// Resource display name.
    pub name: String,
    /// Address range in CIDR notation.
    pub cidr: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl CreateSubnetInput {
    /// Sets the required operation inputs.
    pub fn new(
        vpc_id: impl Into<String>,
        zone_id: impl Into<String>,
        name: impl Into<String>,
        cidr: impl Into<String>,
    ) -> Self {
        Self {
            vpc_id: vpc_id.into(),
            zone_id: zone_id.into(),
            name: name.into(),
            cidr: cidr.into(),
            no_wait: false,
        }
    }
}

/// The created subnet or its last observed state.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CreateSubnetOutput {
    /// The subnet returned by the operation.
    #[serde(rename = "Subnet")]
    pub subnet: Subnet,
}

/// Selects a subnet and its replacement name.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct UpdateSubnetInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
    /// Identifier of the subnet.
    pub subnet_id: String,
    /// Resource display name.
    pub name: String,
}

impl UpdateSubnetInput {
    /// Sets the required operation inputs.
    pub fn new(
        vpc_id: impl Into<String>,
        subnet_id: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        Self {
            vpc_id: vpc_id.into(),
            subnet_id: subnet_id.into(),
            name: name.into(),
        }
    }
}

/// The renamed subnet or submitted name if the read fails.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UpdateSubnetOutput {
    /// The subnet returned by the operation.
    #[serde(rename = "Subnet")]
    pub subnet: Subnet,
}

/// Selects a subnet and whether to wait for its removal.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DeleteSubnetInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
    /// Identifier of the subnet.
    pub subnet_id: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl DeleteSubnetInput {
    /// Sets the required operation inputs.
    pub fn new(vpc_id: impl Into<String>, subnet_id: impl Into<String>) -> Self {
        Self {
            vpc_id: vpc_id.into(),
            subnet_id: subnet_id.into(),
            no_wait: false,
        }
    }
}

/// An accepted subnet deletion has no response fields.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DeleteSubnetOutput {}

/// Selects a VPC and names a new route table.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CreateRouteTableInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
    /// Resource display name.
    pub name: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl CreateRouteTableInput {
    /// Sets the required operation inputs.
    pub fn new(vpc_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            vpc_id: vpc_id.into(),
            name: name.into(),
            no_wait: false,
        }
    }
}

/// The created table or its last observed state.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CreateRouteTableOutput {
    /// The route table returned by the operation.
    #[serde(rename = "RouteTable")]
    pub route_table: RouteTable,
}

/// Selects a route table and whether to wait for its removal.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DeleteRouteTableInput {
    /// Identifier of the route table.
    pub route_table_id: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl DeleteRouteTableInput {
    /// Sets the required operation inputs.
    pub fn new(route_table_id: impl Into<String>) -> Self {
        Self {
            route_table_id: route_table_id.into(),
            no_wait: false,
        }
    }
}

/// An accepted route table deletion has no response fields.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DeleteRouteTableOutput {}

/// Selects a table and defines the destination prefix and next hop.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct AddRouteInput {
    /// Identifier of the route table.
    pub route_table_id: String,
    /// Destination prefix in CIDR notation.
    pub destination_cidr: String,
    /// Next-hop IP address.
    pub target: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl AddRouteInput {
    /// Sets the required operation inputs.
    pub fn new(
        route_table_id: impl Into<String>,
        destination_cidr: impl Into<String>,
        target: impl Into<String>,
    ) -> Self {
        Self {
            route_table_id: route_table_id.into(),
            destination_cidr: destination_cidr.into(),
            target: target.into(),
            no_wait: false,
        }
    }
}

/// The resulting route table and whether a route was added.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AddRouteOutput {
    /// The route table returned by the operation.
    #[serde(rename = "RouteTable")]
    pub route_table: RouteTable,
    /// Whether the operation changed the resource.
    #[serde(rename = "Changed")]
    pub changed: bool,
}

/// Selects a table and the destination prefix to remove.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct RemoveRouteInput {
    /// Identifier of the route table.
    pub route_table_id: String,
    /// Destination prefix in CIDR notation.
    pub destination_cidr: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl RemoveRouteInput {
    /// Sets the required operation inputs.
    pub fn new(route_table_id: impl Into<String>, destination_cidr: impl Into<String>) -> Self {
        Self {
            route_table_id: route_table_id.into(),
            destination_cidr: destination_cidr.into(),
            no_wait: false,
        }
    }
}

/// The resulting route table and whether a route was removed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RemoveRouteOutput {
    /// The route table returned by the operation.
    #[serde(rename = "RouteTable")]
    pub route_table: RouteTable,
    /// Whether the operation changed the resource.
    #[serde(rename = "Changed")]
    pub changed: bool,
}

/// Selects a VPC and names a new ACL.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CreateNetworkAclInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
    /// Resource display name.
    pub name: String,
}

impl CreateNetworkAclInput {
    /// Sets the required operation inputs.
    pub fn new(vpc_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            vpc_id: vpc_id.into(),
            name: name.into(),
        }
    }
}

/// The ACL returned by the service.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CreateNetworkAclOutput {
    /// The ACL returned by the operation.
    #[serde(rename = "ACL")]
    pub acl: NetworkAcl,
}

/// Selects the unattached, nondefault ACL to delete.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DeleteNetworkAclInput {
    /// Identifier of the network ACL.
    pub network_acl_id: String,
}

impl DeleteNetworkAclInput {
    /// Sets the required operation inputs.
    pub fn new(network_acl_id: impl Into<String>) -> Self {
        Self {
            network_acl_id: network_acl_id.into(),
        }
    }
}

/// An accepted ACL deletion has no response fields.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DeleteNetworkAclOutput {}

/// Selects an ACL and defines a traffic rule.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct AddNetworkAclRuleInput {
    /// Identifier of the network ACL.
    pub network_acl_id: String,
    /// Direction of traffic.
    pub direction: String,
    /// Rule sequence number.
    pub priority: i64,
    /// Traffic protocol.
    pub protocol: String,
    /// Address range in CIDR notation.
    pub cidr: String,
    /// Action applied to matching traffic.
    pub action: String,
    /// Lower bound of the port range.
    pub port_range_min: i64,
    /// Upper bound of the port range.
    pub port_range_max: i64,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl AddNetworkAclRuleInput {
    /// Sets the required operation inputs.
    pub fn new(
        network_acl_id: impl Into<String>,
        direction: impl Into<String>,
        priority: i64,
        protocol: impl Into<String>,
        cidr: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            network_acl_id: network_acl_id.into(),
            direction: direction.into(),
            priority,
            protocol: protocol.into(),
            cidr: cidr.into(),
            action: action.into(),
            port_range_min: 0,
            port_range_max: 0,
            no_wait: false,
        }
    }
}

/// The resulting ACL and whether a rule was added.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AddNetworkAclRuleOutput {
    /// The ACL returned by the operation.
    #[serde(rename = "ACL")]
    pub acl: NetworkAcl,
    /// Whether the operation changed the resource.
    #[serde(rename = "Changed")]
    pub changed: bool,
}

/// Selects a rule by ACL, direction and priority.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct RemoveNetworkAclRuleInput {
    /// Identifier of the network ACL.
    pub network_acl_id: String,
    /// Direction of traffic.
    pub direction: String,
    /// Rule sequence number.
    pub priority: i64,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl RemoveNetworkAclRuleInput {
    /// Sets the required operation inputs.
    pub fn new(
        network_acl_id: impl Into<String>,
        direction: impl Into<String>,
        priority: i64,
    ) -> Self {
        Self {
            network_acl_id: network_acl_id.into(),
            direction: direction.into(),
            priority,
            no_wait: false,
        }
    }
}

/// The resulting ACL and whether a rule was removed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RemoveNetworkAclRuleOutput {
    /// The ACL returned by the operation.
    #[serde(rename = "ACL")]
    pub acl: NetworkAcl,
    /// Whether the operation changed the resource.
    #[serde(rename = "Changed")]
    pub changed: bool,
}

/// Selects the ACL and the subnet to associate.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct AssociateNetworkAclSubnetInput {
    /// Identifier of the network ACL.
    pub network_acl_id: String,
    /// Identifier of the subnet.
    pub subnet_id: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl AssociateNetworkAclSubnetInput {
    /// Sets the required operation inputs.
    pub fn new(network_acl_id: impl Into<String>, subnet_id: impl Into<String>) -> Self {
        Self {
            network_acl_id: network_acl_id.into(),
            subnet_id: subnet_id.into(),
            no_wait: false,
        }
    }
}

/// The resulting ACL and identity of the prior ACL if the subnet moved.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AssociateNetworkAclSubnetOutput {
    /// The ACL returned by the operation.
    #[serde(rename = "ACL")]
    pub acl: NetworkAcl,
    /// Whether the operation changed the resource.
    #[serde(rename = "Changed")]
    pub changed: bool,
    /// Identifier of the previous ACL, when an association moved.
    #[serde(rename = "PreviousNetworkACLID")]
    pub previous_network_acl_id: String,
}

/// Selects the ACL and the subnet to disassociate.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DisassociateNetworkAclSubnetInput {
    /// Identifier of the network ACL.
    pub network_acl_id: String,
    /// Identifier of the subnet.
    pub subnet_id: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl DisassociateNetworkAclSubnetInput {
    /// Sets the required operation inputs.
    pub fn new(network_acl_id: impl Into<String>, subnet_id: impl Into<String>) -> Self {
        Self {
            network_acl_id: network_acl_id.into(),
            subnet_id: subnet_id.into(),
            no_wait: false,
        }
    }
}

/// The resulting ACL and whether its subnet association changed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DisassociateNetworkAclSubnetOutput {
    /// The ACL returned by the operation.
    #[serde(rename = "ACL")]
    pub acl: NetworkAcl,
    /// Whether the operation changed the resource.
    #[serde(rename = "Changed")]
    pub changed: bool,
}
