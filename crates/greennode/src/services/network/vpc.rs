use greennode_sdk::network::*;
use greennode_sdk::{Config, RequestContext};

use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};

use super::check_file;

const fn flag(name: &'static str, go_type: &'static str, required: bool) -> Field {
    Field {
        name,
        go_type,
        required,
    }
}

const DHCP_ID: Field = flag("dhcp-options-id", "string", true);
const ACL_ID: Field = flag("network-acl-id", "string", true);
const GROUP_ID: Field = flag("security-group-id", "string", true);
const VPC_ID: Field = flag("vpc-id", "string", true);
const SUBNET_ID: Field = flag("subnet-id", "string", true);
const NAME: Field = flag("name", "string", false);
const PAGE: Field = flag("page", "int", false);
const SIZE: Field = flag("size", "int", false);

const ACL_NOTES: &[&str] = &[
    "Sets `DefaultACL`, `VPCID`, `Rules`, and `SubnetIDs`. `list-network-acls` sets `NetworkID` and `SubnetID`. Fields specific to the other read have zero values.",
    "The detail response must contain explicit ACL state and unique subnet IDs. On a server error, a complete ACL inventory must confirm absence before the command reports `NotFound`.",
];
const ALL_RULES_NOTES: &[&str] = &[
    "Reads every security group page, up to 50 pages and 500000 groups. Repeated IDs or pages, changing totals, and incomplete inventories return `InvalidResponse` without partial output.",
    "Each security group's response must contain an explicit rule array with valid, unique rule IDs.",
];
const SUBNETS_NOTES: &[&str] = &[
    "Reads subnets for the VPCs returned by the default `list-vpcs` page. Use `list-subnets-by-vpc` to read a specific VPC.",
];

macro_rules! required_id {
    ($key:literal, $flag:ident, $field:ident) => {
        nat_field!(
            $key,
            $flag,
            $field,
            Requirement::Nonzero(|input| !input.$field.is_empty())
        )
    };
}

macro_rules! number {
    ($key:literal, $flag:ident, $field:ident) => {
        InputField {
            json_name: $key,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$field = value.as_i64().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$field = *matches.get_one::<i64>($flag.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Optional,
        }
    };
}

macro_rules! operation {
    ($constant:ident, $method:ident, $input:ident, $defaults:expr, $name:literal, $sdk:literal, $short:literal, $example:literal, $notes:expr, $flags:expr, $fields:expr) => {
        pub(super) const $constant: Operation = {
            const INPUT: InputDescriptor<$input> = InputDescriptor {
                defaults: || $defaults,
                fields: $fields,
                allow_json: true,
                allow_query: true,
                project_binding: None,
                pre_guard: Some(check_file),
                guard: None,
            };
            read!(
                $name, $sdk, $short, $example, $flags, $notes, INPUT, $method
            )
        };
        run!($method, $input);
    };
}

operation!(
    GET_DHCP_OPTIONS,
    get_dhcp_options,
    GetDhcpOptionsInput,
    GetDhcpOptionsInput::new(""),
    "get-dhcp-options",
    "network.GetDHCPOptions",
    "Get DHCP options",
    "greennode network get-dhcp-options --dhcp-options-id <dhcp-options-id> --query DHCPOptions",
    &[],
    &[DHCP_ID],
    &[required_id!("DHCPOptionsID", DHCP_ID, dhcp_options_id)]
);

operation!(
    GET_NETWORK_ACL,
    get_network_acl,
    GetNetworkAclInput,
    GetNetworkAclInput::new(""),
    "get-network-acl",
    "network.GetNetworkACL",
    "Get a network ACL",
    "greennode network get-network-acl --network-acl-id <network-acl-id> --query ACL",
    ACL_NOTES,
    &[ACL_ID],
    &[required_id!("NetworkACLID", ACL_ID, network_acl_id)]
);

operation!(
    GET_SECURITY_GROUP,
    get_security_group,
    GetSecurityGroupInput,
    GetSecurityGroupInput::new(""),
    "get-security-group",
    "network.GetSecurityGroup",
    "Get a security group",
    "greennode network get-security-group --security-group-id <security-group-id> --query SecurityGroup",
    &[],
    &[GROUP_ID],
    &[required_id!("SecurityGroupID", GROUP_ID, security_group_id)]
);

operation!(
    GET_SUBNET,
    get_subnet,
    GetSubnetInput,
    GetSubnetInput::new("", ""),
    "get-subnet",
    "network.GetSubnet",
    "Get a subnet",
    "greennode network get-subnet --vpc-id <vpc-id> --subnet-id <subnet-id> --query Subnet",
    &[],
    &[VPC_ID, SUBNET_ID],
    &[
        required_id!("VPCID", VPC_ID, vpc_id),
        required_id!("SubnetID", SUBNET_ID, subnet_id)
    ]
);

operation!(
    GET_VPC,
    get_vpc,
    GetVpcInput,
    GetVpcInput::new(""),
    "get-vpc",
    "network.GetVPC",
    "Get a VPC",
    "greennode network get-vpc --vpc-id <vpc-id> --query VPC",
    &[],
    &[VPC_ID],
    &[required_id!("VPCID", VPC_ID, vpc_id)]
);

operation!(
    LIST_ALL_SECURITY_GROUP_RULES,
    list_all_security_group_rules,
    ListAllSecurityGroupRulesInput,
    ListAllSecurityGroupRulesInput::default(),
    "list-all-security-group-rules",
    "network.ListAllSecurityGroupRules",
    "List rules across all security groups",
    "greennode network list-all-security-group-rules",
    ALL_RULES_NOTES,
    &[],
    &[]
);

operation!(
    LIST_DHCP_OPTIONS,
    list_dhcp_options,
    ListDhcpOptionsInput,
    ListDhcpOptionsInput::default(),
    "list-dhcp-options",
    "network.ListDHCPOptions",
    "List DHCP options sets",
    "greennode network list-dhcp-options",
    &[],
    &[NAME, PAGE, SIZE],
    &[
        nat_field!("Name", NAME, name, Requirement::Optional),
        number!("Page", PAGE, page),
        number!("Size", SIZE, size)
    ]
);

operation!(
    LIST_NETWORK_ACLS,
    list_network_acls,
    ListNetworkAclsInput,
    ListNetworkAclsInput::default(),
    "list-network-acls",
    "network.ListNetworkACLs",
    "List network ACLs",
    "greennode network list-network-acls",
    &[],
    &[NAME, PAGE, SIZE],
    &[
        nat_field!("Name", NAME, name, Requirement::Optional),
        number!("Page", PAGE, page),
        number!("Size", SIZE, size)
    ]
);

operation!(
    LIST_SECURITY_GROUP_RULES,
    list_security_group_rules,
    ListSecurityGroupRulesInput,
    ListSecurityGroupRulesInput::new(""),
    "list-security-group-rules",
    "network.ListSecurityGroupRules",
    "List rules in a security group",
    "greennode network list-security-group-rules --security-group-id <security-group-id>",
    &[],
    &[GROUP_ID],
    &[required_id!("SecurityGroupID", GROUP_ID, security_group_id)]
);

operation!(
    LIST_SECURITY_GROUPS,
    list_security_groups,
    ListSecurityGroupsInput,
    ListSecurityGroupsInput::default(),
    "list-security-groups",
    "network.ListSecurityGroups",
    "List security groups",
    "greennode network list-security-groups",
    &[],
    &[NAME, PAGE, SIZE],
    &[
        nat_field!("Name", NAME, name, Requirement::Optional),
        number!("Page", PAGE, page),
        number!("Size", SIZE, size)
    ]
);

operation!(
    LIST_SUBNETS,
    list_subnets,
    ListSubnetsInput,
    ListSubnetsInput::default(),
    "list-subnets",
    "network.ListSubnets",
    "List subnets across VPCs",
    "greennode network list-subnets",
    SUBNETS_NOTES,
    &[],
    &[]
);

operation!(
    LIST_SUBNETS_BY_VPC,
    list_subnets_by_vpc,
    ListSubnetsByVpcInput,
    ListSubnetsByVpcInput::new(""),
    "list-subnets-by-vpc",
    "network.ListSubnetsByVPC",
    "List subnets within a VPC",
    "greennode network list-subnets-by-vpc --vpc-id <vpc-id>",
    &[],
    &[VPC_ID],
    &[required_id!("VPCID", VPC_ID, vpc_id)]
);

operation!(
    LIST_VPCS,
    list_vpcs,
    ListVpcsInput,
    ListVpcsInput::default(),
    "list-vpcs",
    "network.ListVPCs",
    "List VPCs",
    "greennode network list-vpcs",
    &[],
    &[NAME, PAGE, SIZE],
    &[
        nat_field!("Name", NAME, name, Requirement::Optional),
        number!("Page", PAGE, page),
        number!("Size", SIZE, size)
    ]
);
