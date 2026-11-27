use greennode_sdk::network::{
    GetEndpointInput, ListEndpointTagsInput, ListEndpointsInput, ListServersBySecurityGroupInput,
    ListServersBySubnetInput, ListVNetworkRegionsInput,
};

use super::*;

const ENDPOINT_ID: Field = Field {
    name: "endpoint-id",
    go_type: "string",
    required: true,
};
const SECURITY_GROUP_ID: Field = Field {
    name: "security-group-id",
    go_type: "string",
    required: true,
};
const SUBNET_ID: Field = Field {
    name: "subnet-id",
    go_type: "string",
    required: true,
};
const VPC_ID: Field = Field {
    name: "vpc-id",
    go_type: "string",
    required: false,
};
const UUID: Field = Field {
    name: "uuid",
    go_type: "string",
    required: false,
};
const PAGE: Field = Field {
    name: "page",
    go_type: "int",
    required: false,
};
const SIZE: Field = Field {
    name: "size",
    go_type: "int",
    required: false,
};

macro_rules! one_id {
    ($type:ident, $json:literal, $flag:ident, $field:ident) => {
        InputDescriptor {
            defaults: || $type::new(""),
            fields: &[nat_field!(
                $json,
                $flag,
                $field,
                Requirement::Nonzero(|input| !input.$field.is_empty())
            )],
            allow_json: true,
            allow_query: true,
            project_binding: None,
            pre_guard: Some(check_file),
            guard: None,
        }
    };
}

const DETAIL_INPUT: InputDescriptor<GetEndpointInput> =
    one_id!(GetEndpointInput, "EndpointID", ENDPOINT_ID, endpoint_id);
const TAGS_INPUT: InputDescriptor<ListEndpointTagsInput> = one_id!(
    ListEndpointTagsInput,
    "EndpointID",
    ENDPOINT_ID,
    endpoint_id
);
const GROUP_INPUT: InputDescriptor<ListServersBySecurityGroupInput> = one_id!(
    ListServersBySecurityGroupInput,
    "SecurityGroupID",
    SECURITY_GROUP_ID,
    security_group_id
);
const SUBNET_INPUT: InputDescriptor<ListServersBySubnetInput> =
    one_id!(ListServersBySubnetInput, "SubnetID", SUBNET_ID, subnet_id);
const REGION_INPUT: InputDescriptor<ListVNetworkRegionsInput> = InputDescriptor {
    defaults: ListVNetworkRegionsInput::default,
    fields: &[],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_file),
    guard: None,
};
const LIST_INPUT: InputDescriptor<ListEndpointsInput> = InputDescriptor {
    defaults: ListEndpointsInput::default,
    fields: &[
        nat_field!("ZoneID", ZONE_ID, zone_id, Requirement::Optional),
        nat_field!("VPCID", VPC_ID, vpc_id, Requirement::Optional),
        nat_field!("UUID", UUID, uuid, Requirement::Optional),
        integer!("Page", PAGE, page),
        integer!("Size", SIZE, size),
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_file),
    guard: None,
};

pub(super) const GET_ENDPOINT: Operation = read!(
    "get-endpoint",
    "network.GetEndpoint",
    "Get endpoint details",
    "greennode network get-endpoint --endpoint-id <endpoint-id> --query Endpoint",
    &[ENDPOINT_ID],
    &[],
    DETAIL_INPUT,
    get_endpoint
);
pub(super) const LIST_ENDPOINT_TAGS: Operation = read!(
    "list-endpoint-tags",
    "network.ListEndpointTags",
    "List endpoint tags",
    "greennode network list-endpoint-tags --endpoint-id <endpoint-id>",
    &[ENDPOINT_ID],
    &[],
    TAGS_INPUT,
    list_endpoint_tags
);
pub(super) const LIST_ENDPOINTS: Operation = read!(
    "list-endpoints",
    "network.ListEndpoints",
    "List endpoints",
    "greennode network list-endpoints",
    &[ZONE_ID, VPC_ID, UUID, PAGE, SIZE],
    &[],
    LIST_INPUT,
    list_endpoints
);
pub(super) const SERVERS_BY_GROUP: Operation = read!(
    "list-servers-by-security-group",
    "network.ListServersBySecurityGroup",
    "List servers using a security group",
    "greennode network list-servers-by-security-group --security-group-id <security-group-id>",
    &[SECURITY_GROUP_ID],
    &[
        "PublicIP is the floating address derived from internal interfaces. Select addresses with `--query 'Items[].PublicIP'`.",
        "Responses are limited to 8 MiB. The SDK suppresses response captures and withholds unsafe server data.",
    ],
    GROUP_INPUT,
    list_servers_by_security_group
);
pub(super) const SERVERS_BY_SUBNET: Operation = read!(
    "list-servers-by-subnet",
    "network.ListServersBySubnet",
    "List servers attached to a subnet",
    "greennode network list-servers-by-subnet --subnet-id <subnet-id>",
    &[SUBNET_ID],
    &[],
    SUBNET_INPUT,
    list_servers_by_subnet
);
pub(super) const REGIONS: Operation = read!(
    "list-v-network-regions",
    "network.ListVNetworkRegions",
    "List vNetwork regions",
    "greennode network list-v-network-regions",
    &[],
    &[
        "Requests use the fixed regional gateway or the caller's explicit endpoint override. Discovered data cannot select a credential destination.",
    ],
    REGION_INPUT,
    list_v_network_regions
);

run!(get_endpoint, GetEndpointInput);
run!(list_endpoint_tags, ListEndpointTagsInput);
run!(list_endpoints, ListEndpointsInput);
run!(
    list_servers_by_security_group,
    ListServersBySecurityGroupInput
);
run!(list_servers_by_subnet, ListServersBySubnetInput);
run!(list_v_network_regions, ListVNetworkRegionsInput);
