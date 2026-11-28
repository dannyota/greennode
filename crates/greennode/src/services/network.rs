mod routes;

use clap::ArgMatches;
use greennode_sdk::network::{
    Client, GetNatInstanceInput, ListBandwidthPackagesInput, ListNatInboundRulesInput,
};
use greennode_sdk::{Config, RequestContext};

use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, Service};

const NAT_ID: Field = Field {
    name: "nat-id",
    go_type: "string",
    required: true,
};
const ZONE_ID: Field = Field {
    name: "zone-id",
    go_type: "string",
    required: false,
};

fn check_file(_: &ArgMatches, json: Option<&str>) -> Result<(), InputError> {
    if json == Some("file://") {
        return Err(InputError("--cli-input-json: cannot open file"));
    }
    Ok(())
}

macro_rules! nat_input {
    ($type:ident) => {
        InputDescriptor {
            defaults: || $type::new(""),
            fields: &[
                nat_field!(
                    "NATID",
                    NAT_ID,
                    nat_id,
                    Requirement::Nonzero(|input| !input.nat_id.is_empty())
                ),
                nat_field!("ZoneID", ZONE_ID, zone_id, Requirement::Optional),
            ],
            allow_json: true,
            allow_query: true,
            project_binding: None,
            pre_guard: Some(check_file),
            guard: None,
        }
    };
}

macro_rules! nat_field {
    ($json:literal, $flag:ident, $field:ident, $requirement:expr) => {
        InputField {
            json_name: $json,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                let serde_json::Value::String(value) = value else {
                    return Err(());
                };
                input.$field = value;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$field = matches.get_one::<String>($flag.name).ok_or(())?.clone();
                Ok(())
            }),
            requirement: $requirement,
        }
    };
}

const DETAIL_INPUT: InputDescriptor<GetNatInstanceInput> = nat_input!(GetNatInstanceInput);
const RULES_INPUT: InputDescriptor<ListNatInboundRulesInput> = nat_input!(ListNatInboundRulesInput);
const BANDWIDTH_INPUT: InputDescriptor<ListBandwidthPackagesInput> = InputDescriptor {
    defaults: ListBandwidthPackagesInput::default,
    fields: &[],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_file),
    guard: None,
};

const NAT_NOTES: &[&str] = &[
    "Reads in `hcm-3` or `han-1` with IAM or service-account authentication. Read-only profiles can run this command.",
    "An omitted `--zone-id` requires a unique zone mapping for the selected region.",
];

macro_rules! read {
    ($command:literal, $sdk:literal, $help:literal, $example:literal, $flags:expr, $notes:expr, $input:ident, $run:ident) => {
        Operation {
            name: $command,
            sdk_operation: $sdk,
            short_help: $help,
            example: Some($example),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: $flags,
            json_fields: &[],
            extra_flags: &[],
            notes: $notes,
            handler: Some(Handler::new(&$input, |operation, matches, invocation| {
                Box::pin(dispatch::management(
                    invocation, operation, matches, &$input, $run,
                ))
            })),
        }
    };
}

macro_rules! run {
    ($method:ident, $input:ident) => {
        fn $method<'a>(
            ctx: &'a RequestContext,
            config: &'a Config,
            input: &'a $input,
        ) -> FutureResult<'a, ExecutionResult> {
            Box::pin(async move {
                dispatch::project_result(
                    Client::new(config).$method(ctx, input.clone()).await,
                    std::convert::identity,
                )
            })
        }
    };
}

macro_rules! integer {
    ($json:literal, $flag:ident, $field:ident) => {
        InputField {
            json_name: $json,
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

mod catalog;
mod inventory;
mod prices;
mod quotes;
mod resource_writes;

pub(super) const SERVICE: Service = Service {
    name: "network",
    error_rules: &[
        prices::DISCOVERY_INCOMPLETE,
        resource_writes::SYSTEM_GROUP,
        resource_writes::GROUP_IN_USE,
        resource_writes::RESOURCE_IN_USE,
        writes::UNEXPECTEDSTATUS,
        writes::DEFAULTRESOURCE,
        writes::RESOURCEBUSY,
    ],
    operations: &[
        resource_writes::CREATE_SECURITY_GROUP,
        resource_writes::UPDATE_SECURITY_GROUP,
        resource_writes::DELETE_SECURITY_GROUP,
        resource_writes::CREATE_SECURITY_GROUP_RULE,
        resource_writes::DELETE_SECURITY_GROUP_RULE,
        resource_writes::CREATE_VIRTUAL_IP_ADDRESS,
        resource_writes::UPDATE_VIRTUAL_IP_ADDRESS,
        resource_writes::DELETE_VIRTUAL_IP_ADDRESS,
        resource_writes::CREATE_ENDPOINT,
        resource_writes::DELETE_ENDPOINT,
        resource_writes::CREATE_NAT_INSTANCE,
        resource_writes::DELETE_NAT_INSTANCE,
        routes::GET_ROUTE_TABLE,
        routes::GET_VIRTUAL_IP_ADDRESS,
        routes::LIST_PAIRS_BY_VIP,
        routes::LIST_PAIRS_BY_SUBNET,
        routes::LIST_ALL_VIP_PAIRS,
        routes::LIST_INTERCONNECTS,
        routes::LIST_NETWORK_INTERFACES,
        routes::LIST_PEERINGS,
        routes::LIST_ROUTE_TABLE_ROUTES,
        routes::LIST_ROUTE_TABLES,
        routes::LIST_VIRTUAL_IP_ADDRESSES,
        routes::LIST_VPN_CONNECTIONS,
        routes::LIST_WAN_IPS,
        quotes::ENDPOINT,
        quotes::NAT,
        prices::OPERATION,
        catalog::INSTANCES,
        catalog::PACKAGES,
        catalog::ZONES,
        inventory::GET_ENDPOINT,
        inventory::LIST_ENDPOINT_TAGS,
        inventory::LIST_ENDPOINTS,
        inventory::SERVERS_BY_GROUP,
        inventory::SERVERS_BY_SUBNET,
        inventory::REGIONS,
        read!(
            "get-nat-instance",
            "network.GetNATInstance",
            "Get a Public NAT instance",
            "greennode network get-nat-instance --nat-id <nat-id>",
            &[NAT_ID, ZONE_ID],
            NAT_NOTES,
            DETAIL_INPUT,
            get_nat_instance
        ),
        read!(
            "list-bandwidth-packages",
            "network.ListBandwidthPackages",
            "List regional bandwidth packages",
            "greennode network list-bandwidth-packages",
            &[],
            &[],
            BANDWIDTH_INPUT,
            list_bandwidth_packages
        ),
        read!(
            "list-nat-inbound-rules",
            "network.ListNATInboundRules",
            "List a Public NAT instance's inbound rules",
            "greennode network list-nat-inbound-rules --nat-id <nat-id>",
            &[NAT_ID, ZONE_ID],
            NAT_NOTES,
            RULES_INPUT,
            list_nat_inbound_rules
        ),
        vpc::GET_DHCP_OPTIONS,
        vpc::GET_NETWORK_ACL,
        vpc::GET_SECURITY_GROUP,
        vpc::GET_SUBNET,
        vpc::GET_VPC,
        vpc::LIST_ALL_SECURITY_GROUP_RULES,
        vpc::LIST_DHCP_OPTIONS,
        vpc::LIST_NETWORK_ACLS,
        vpc::LIST_SECURITY_GROUP_RULES,
        vpc::LIST_SECURITY_GROUPS,
        vpc::LIST_SUBNETS,
        vpc::LIST_SUBNETS_BY_VPC,
        vpc::LIST_VPCS,
        writes::vpcs::CREATE_VPC,
        writes::vpcs::UPDATE_VPC,
        writes::vpcs::DELETE_VPC,
        writes::vpcs::ENABLE_VPC_PRIVATE_DNS,
        writes::dhcp::CREATE_DHCP_OPTIONS,
        writes::dhcp::DELETE_DHCP_OPTIONS,
        writes::dhcp::SET_VPC_DHCP_OPTIONS,
        writes::dhcp::CLEAR_VPC_DHCP_OPTIONS,
        writes::subnets::CREATE_SUBNET,
        writes::subnets::UPDATE_SUBNET,
        writes::subnets::DELETE_SUBNET,
        writes::routes::CREATE_ROUTE_TABLE,
        writes::routes::DELETE_ROUTE_TABLE,
        writes::routes::ADD_ROUTE,
        writes::routes::REMOVE_ROUTE,
        writes::acls::CREATE_NETWORK_ACL,
        writes::acls::DELETE_NETWORK_ACL,
        writes::acls::ADD_NETWORK_ACL_RULE,
        writes::acls::REMOVE_NETWORK_ACL_RULE,
        writes::acls::ASSOCIATE_NETWORK_ACL_SUBNET,
        writes::acls::DISASSOCIATE_NETWORK_ACL_SUBNET,
    ],
};

run!(get_nat_instance, GetNatInstanceInput);
run!(list_bandwidth_packages, ListBandwidthPackagesInput);
run!(list_nat_inbound_rules, ListNatInboundRulesInput);

mod vpc;
mod writes;
