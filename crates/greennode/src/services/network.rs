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

pub(super) const SERVICE: Service = Service {
    name: "network",
    error_rules: &[],
    operations: &[
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
    ],
};

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

run!(get_nat_instance, GetNatInstanceInput);
run!(list_bandwidth_packages, ListBandwidthPackagesInput);
run!(list_nat_inbound_rules, ListNatInboundRulesInput);

mod vpc;
