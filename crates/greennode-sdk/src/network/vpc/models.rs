use serde::Serialize;

use crate::model::wire::model;
use crate::{List, PagedList};

model! {
    @ true;
    /// A virtual private cloud.
    pub struct Vpc {
        /// Resource identifier.
        uuid: String => "id", "UUID";
        /// Service-reported status.
        status: String => "status", "Status";
        /// Assigned elastic IP addresses.
        elastic_ips: Vec<String> => "elasticIps", "ElasticIPs";
        /// Display name.
        name: String => "displayName", "Name";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Address range in CIDR notation.
        cidr: String => "cidr", "CIDR";
        /// Attached DHCP options set name.
        dhcp_option_name: String => "dhcpOptionName", "DHCPOptionName";
        /// Attached DHCP options set ID.
        dhcp_option_id: String => "dhcpOptionId", "DHCPOptionID";
        /// Associated route table name.
        route_table_name: String => "routeTableName", "RouteTableName";
        /// Associated route table ID.
        route_table_id: String => "routeTableId", "RouteTableID";
        /// Embedded regional zone.
        zone: Zone => "zone", "Zone";
        /// Private DNS status.
        dns_status: String => "dnsStatus", "DNSStatus";
        /// Private DNS identifier.
        dns_id: String => "dnsId", "DNSID";
        /// Maximum transmission unit.
        mtu: i64 => "mtu", "MTU";
        /// Number of servers.
        server_count: i64 => "serverCount", "ServerCount";
        /// Number of volumes.
        volume_count: i64 => "volumeCount", "VolumeCount";
    }
}

model! {
    @ true;
    /// A subnet within a VPC.
    pub struct Subnet {
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
        /// Display name.
        name: String => "name", "Name";
        /// Associated network identifier.
        network_id: String => "networkId", "NetworkID";
        /// Associated network UUID.
        network_uuid: String => "networkUuid", "NetworkUUID";
        /// Address range in CIDR notation.
        cidr: String => "cidr", "CIDR";
        /// Service-reported status.
        status: String => "status", "Status";
        /// Interface ACL policy ID.
        interface_acl_policy_id: String => "interfaceAclPolicyId", "InterfaceACLPolicyID";
        /// Interface ACL policy UUID.
        interface_acl_policy_uuid: String => "interfaceAclPolicyUuid", "InterfaceACLPolicyUUID";
        /// Interface ACL policy name.
        interface_acl_policy_name: String => "interfaceAclPolicyName", "InterfaceACLPolicyName";
        /// Associated route table ID.
        route_table_id: String => "routeTableId", "RouteTableID";
        /// Associated route table UUID.
        route_table_uuid: String => "routeTableUuid", "RouteTableUUID";
        /// Regional zone ID.
        zone_id: String => "zoneId", "ZoneID";
        /// Embedded regional zone.
        zone: Zone => "zone", "Zone";
        /// Resource description.
        description: String => "description", "Description";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Last update timestamp.
        updated_at: String => "updatedAt", "UpdatedAt";
        /// Number of servers.
        server_count: i64 => "serverCount", "ServerCount";
        /// Number of volumes.
        volume_count: i64 => "volumeCount", "VolumeCount";
        /// Secondary address ranges.
        secondary_subnets: Vec<SubnetSecondarySubnet> => "secondarySubnets", "SecondarySubnets";
    }
}

model! {
    @ true;
    /// A subnet CIDR added to a primary subnet.
    pub struct SubnetSecondarySubnet {
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
        /// Display name.
        name: String => "name", "Name";
        /// Address range in CIDR notation.
        cidr: String => "cidr", "CIDR";
    }
}

model! {
    @ true;
    /// A regional DHCP options set and its VPC attachments.
    pub struct DhcpOptions {
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
        /// Display name.
        name: String => "name", "Name";
        /// Service-reported status.
        status: String => "status", "Status";
        /// DNS server addresses.
        dns_servers: Vec<String> => "dnsServers", "DNSServers";
        /// Maximum transmission unit.
        mtu: i64 => "mtu", "MTU";
        /// VPCs attached to this options set.
        vpc_ids: Vec<String> => "associatedNetworks", "VPCIDs";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Last update timestamp.
        updated_at: String => "updatedAt", "UpdatedAt";
    }
}

model! {
    @ true;
    /// A network access control list.
    pub struct NetworkAcl {
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
        /// Provider identifier.
        id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// Resource description.
        description: String => "description", "Description";
        /// Service-reported status.
        status: String => "status", "Status";
        /// Associated network identifier.
        network_id: String => "networkId", "NetworkID";
        /// Associated subnet ID.
        subnet_id: String => "subnetId", "SubnetID";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Whether the ACL is the default for its VPC.
        default_acl: bool => "defaultAcl", "DefaultACL";
        /// Associated VPC ID.
        vpc_id: String => "interfaceNetworkUuid", "VPCID";
        /// Rules returned by the detail read.
        rules: Vec<NetworkAclRule> => "aclPolicyRules", "Rules";
        /// Subnets associated with the ACL.
        subnet_ids: Vec<String> => "subnetAssociationList", "SubnetIDs";
    }
}

model! {
    @ true;
    /// A rule in a network access control list.
    pub struct NetworkAclRule {
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
        /// Traffic direction.
        direction: String => "type", "Direction";
        /// Rule sequence number.
        priority: i64 => "seqNumber", "Priority";
        /// Traffic protocol.
        protocol: String => "protocol", "Protocol";
        /// Port range as reported by the service.
        port: String => "port", "Port";
        /// Address range in CIDR notation.
        cidr: String => "source", "CIDR";
        /// Rule action.
        action: String => "action", "Action";
        /// Whether the resource belongs to the system.
        system: bool => "system", "System";
    }
}

model! {
    @ true;
    /// A security group in the project.
    pub struct SecurityGroup {
        /// Provider identifier.
        id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// Resource description.
        description: String => "description", "Description";
        /// Service-reported status.
        status: String => "status", "Status";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// System ownership flag.
        is_system: bool => "isSystem", "IsSystem";
        /// Whether the resource belongs to the system.
        system: bool => "system", "System";
    }
}

model! {
    @ true;
    /// A rule and its parent security group.
    pub struct SecurityGroupRule {
        /// Provider identifier.
        id: String => "id", "ID";
        /// Parent security group ID.
        security_group_id: String => "securityGroupId", "SecurityGroupID";
        /// Underlying rule identifier.
        rule_id: String => "ruleId", "RuleID";
        /// Traffic direction.
        direction: String => "direction", "Direction";
        /// Network protocol family.
        ether_type: String => "etherType", "EtherType";
        /// Traffic protocol.
        protocol: String => "protocol", "Protocol";
        /// Lower port bound.
        port_range_min: i64 => "portRangeMin", "PortRangeMin";
        /// Upper port bound.
        port_range_max: i64 => "portRangeMax", "PortRangeMax";
        /// Remote CIDR range.
        remote_ip_prefix: String => "remoteIpPrefix", "RemoteIPPrefix";
        /// Remote security group ID.
        remote_group_id: String => "remoteGroupId", "RemoteGroupID";
        /// Service-reported status.
        status: String => "status", "Status";
        /// Resource description.
        description: String => "description", "Description";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
    }
}

macro_rules! page_input {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Default)]
        #[non_exhaustive]
        pub struct $name {
            /// Resource name filter.
            pub name: String,
            /// One-based page number. Nonpositive values use the default.
            pub page: i64,
            /// Requested page size. Nonpositive values use the default.
            pub size: i64,
        }
    };
}

page_input!(
    /// Filters and pagination for listing VPCs.
    ListVpcsInput
);
page_input!(
    /// Filters and pagination for listing DHCP options.
    ListDhcpOptionsInput
);
page_input!(
    /// Filters and pagination for listing network ACLs.
    ListNetworkAclsInput
);
page_input!(
    /// Filters and pagination for listing security groups.
    ListSecurityGroupsInput
);

macro_rules! id_input {
    ($(#[$doc:meta])* $name:ident { $($(#[$field_doc:meta])* $field:ident),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Clone, Debug)]
        #[non_exhaustive]
        pub struct $name {
            $($(#[$field_doc])* pub $field: String,)+
        }

        impl $name {
            /// Creates the input with its required identifiers.
            pub fn new($($field: impl Into<String>),+) -> Self {
                Self { $($field: $field.into(),)+ }
            }
        }
    };
}

id_input!(
    /// Identifies a VPC to read.
    GetVpcInput {
        /// VPC identifier.
        vpc_id,
    }
);
id_input!(
    /// Identifies a subnet within a VPC.
    GetSubnetInput {
        /// VPC identifier.
        vpc_id,
        /// Subnet identifier.
        subnet_id,
    }
);
id_input!(
    /// Identifies a VPC whose subnets should be listed.
    ListSubnetsByVpcInput {
        /// VPC identifier.
        vpc_id,
    }
);
id_input!(
    /// Identifies a DHCP options set to read.
    GetDhcpOptionsInput {
        /// DHCP options identifier.
        dhcp_options_id,
    }
);
id_input!(
    /// Identifies a network ACL to read.
    GetNetworkAclInput {
        /// Network ACL identifier.
        network_acl_id,
    }
);
id_input!(
    /// Identifies a security group to read.
    GetSecurityGroupInput {
        /// Security group identifier.
        security_group_id,
    }
);
id_input!(
    /// Identifies a security group whose rules should be listed.
    ListSecurityGroupRulesInput {
        /// Security group identifier.
        security_group_id,
    }
);

/// Lists subnets across the VPCs in the default inventory page.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct ListSubnetsInput {}

/// Lists rules across the project's complete security group inventory.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct ListAllSecurityGroupRulesInput {}

/// One page of VPCs.
pub type ListVpcsOutput = PagedList<Vpc>;
/// One page of DHCP options sets.
pub type ListDhcpOptionsOutput = PagedList<DhcpOptions>;
/// One page of network ACLs.
pub type ListNetworkAclsOutput = PagedList<NetworkAcl>;
/// One page of security groups.
pub type ListSecurityGroupsOutput = PagedList<SecurityGroup>;
/// Subnets across the VPCs in the default inventory page.
pub type ListSubnetsOutput = List<Subnet>;
/// Subnets within the selected VPC.
pub type ListSubnetsByVpcOutput = List<Subnet>;
/// Rules within the selected security group.
pub type ListSecurityGroupRulesOutput = List<SecurityGroupRule>;
/// Rules across all security groups in the project.
pub type ListAllSecurityGroupRulesOutput = List<SecurityGroupRule>;

/// VPC detail.
#[derive(Clone, PartialEq, Serialize)]
pub struct GetVpcOutput {
    /// The VPC.
    #[serde(rename = "VPC")]
    pub vpc: Vpc,
}

crate::wire::product::output::public_fields!(GetVpcOutput { vpc });

/// Subnet detail.
#[derive(Clone, PartialEq, Serialize)]
pub struct GetSubnetOutput {
    /// The subnet.
    #[serde(rename = "Subnet")]
    pub subnet: Subnet,
}

crate::wire::product::output::public_fields!(GetSubnetOutput { subnet });

/// DHCP options detail.
#[derive(Clone, PartialEq, Serialize)]
pub struct GetDhcpOptionsOutput {
    /// The DHCP options set.
    #[serde(rename = "DHCPOptions")]
    pub dhcp_options: DhcpOptions,
}

crate::wire::product::output::public_fields!(GetDhcpOptionsOutput { dhcp_options });

/// Network ACL detail.
#[derive(Clone, PartialEq, Serialize)]
pub struct GetNetworkAclOutput {
    /// The network ACL.
    #[serde(rename = "ACL")]
    pub acl: NetworkAcl,
}

crate::wire::product::output::public_fields!(GetNetworkAclOutput { acl });

/// Security group detail.
#[derive(Clone, PartialEq, Serialize)]
pub struct GetSecurityGroupOutput {
    /// The security group.
    #[serde(rename = "SecurityGroup")]
    pub security_group: SecurityGroup,
}

crate::wire::product::output::public_fields!(GetSecurityGroupOutput { security_group });

model! {
    @ true;
    /// The network zone embedded in a VPC or subnet.
    pub struct Zone {
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
        /// Display name.
        name: String => "name", "Name";
        /// Zone type.
        zone_type: String => "zoneType", "ZoneType";
        /// Is default.
        is_default: bool => "isDefault", "IsDefault";
        /// Resource description.
        description: String => "description", "Description";
        /// Is enabled.
        is_enabled: bool => "isEnabled", "IsEnabled";
        /// Open stack zone.
        openstack_zone: String => "openstackZone", "OpenStackZone";
        /// Ip ranges.
        #[serde(serialize_with = "crate::model::wire::serialize_slice")]
        ip_ranges: Option<Vec<String>> => "ipRanges", "IPRanges";
        /// Server count.
        server_count: i64 => "serverCount", "ServerCount";
        /// Volume count.
        volume_count: i64 => "volumeCount", "VolumeCount";
    }
}
