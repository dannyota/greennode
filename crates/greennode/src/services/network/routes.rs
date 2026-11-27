use greennode_sdk::network as sdk;

use crate::cli::dispatch;
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

const ROUTE_TABLE_ID: Field = flag("route-table-id", "string", true);
const VIP_ID: Field = flag("virtual-ip-address-id", "string", true);
const SUBNET_ID: Field = flag("virtual-subnet-id", "string", true);
const NAME: Field = flag("name", "string", false);
const PAGE: Field = flag("page", "int", false);
const SIZE: Field = flag("size", "int", false);

macro_rules! string_field {
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

macro_rules! int_field {
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

macro_rules! read {
    ($input:ident, $defaults:expr, $method:ident, $command:literal, $go:literal, $help:literal,
     $example:expr, $notes:expr, [$($flag:ident => $field:expr),* $(,)?]) => {{
        const INPUT: InputDescriptor<sdk::$input> = InputDescriptor {
            defaults: $defaults,
            fields: &[$($field),*],
            allow_json: true,
            allow_query: true,
            project_binding: None,
            pre_guard: Some(check_file),
            guard: None,
        };
        Operation {
            name: $command,
            sdk_operation: concat!("network.", $go),
            short_help: $help,
            example: $example,
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[$($flag),*],
            json_fields: &[],
            extra_flags: &[],
            notes: $notes,
            handler: Some(Handler::new(&INPUT, |operation, matches, invocation| {
                Box::pin(dispatch::management(invocation, operation, matches, &INPUT, |ctx, config, input| {
                    Box::pin(async move {
                        dispatch::project_result(
                            sdk::Client::new(config).$method(ctx, input.clone()).await,
                            std::convert::identity,
                        )
                    })
                }))
            })),
        }
    }};
}

macro_rules! by_id {
    ($input:ident, $method:ident, $command:literal, $go:literal, $help:literal, $example:literal,
     $flag:ident, $field:ident, $json:literal) => {
        read!(
            $input, || sdk::$input::new(""), $method, $command, $go, $help, Some($example), &[],
            [$flag => string_field!($json, $flag, $field, Requirement::Nonzero(|input| !input.$field.is_empty()))]
        )
    };
}

macro_rules! paged {
    ($input:ident, $method:ident, $command:literal, $go:literal, $help:literal) => {
        read!(
            $input, sdk::$input::default, $method, $command, $go, $help, None, &[],
            [
                NAME => string_field!("Name", NAME, name, Requirement::Optional),
                PAGE => int_field!("Page", PAGE, page),
                SIZE => int_field!("Size", SIZE, size),
            ]
        )
    };
}

pub(super) const GET_ROUTE_TABLE: Operation = by_id!(
    GetRouteTableInput,
    get_route_table,
    "get-route-table",
    "GetRouteTable",
    "Get a route table",
    "greennode network get-route-table --route-table-id <route-table-id> --query RouteTable",
    ROUTE_TABLE_ID,
    route_table_id,
    "RouteTableID"
);

pub(super) const GET_VIRTUAL_IP_ADDRESS: Operation = by_id!(
    GetVirtualIpAddressInput,
    get_virtual_ip_address,
    "get-virtual-ip-address",
    "GetVirtualIPAddress",
    "Get a virtual IP address",
    "greennode network get-virtual-ip-address --virtual-ip-address-id <virtual-ip-address-id> --query VirtualIPAddress",
    VIP_ID,
    virtual_ip_address_id,
    "VirtualIPAddressID"
);

pub(super) const LIST_PAIRS_BY_VIP: Operation = by_id!(
    ListAddressPairsByVirtualIpAddressInput,
    list_address_pairs_by_virtual_ip_address,
    "list-address-pairs-by-virtual-ip-address",
    "ListAddressPairsByVirtualIPAddress",
    "List address pairs for a virtual IP address",
    "greennode network list-address-pairs-by-virtual-ip-address --virtual-ip-address-id <virtual-ip-address-id>",
    VIP_ID,
    virtual_ip_address_id,
    "VirtualIPAddressID"
);

pub(super) const LIST_PAIRS_BY_SUBNET: Operation = by_id!(
    ListAddressPairsByVirtualSubnetInput,
    list_address_pairs_by_virtual_subnet,
    "list-address-pairs-by-virtual-subnet",
    "ListAddressPairsByVirtualSubnet",
    "List address pairs for a virtual subnet",
    "greennode network list-address-pairs-by-virtual-subnet --virtual-subnet-id <virtual-subnet-id>",
    SUBNET_ID,
    virtual_subnet_id,
    "VirtualSubnetID"
);

pub(super) const LIST_ALL_VIP_PAIRS: Operation = read!(
    ListAllVirtualIpAddressAddressPairsInput,
    sdk::ListAllVirtualIpAddressAddressPairsInput::default,
    list_all_virtual_ip_address_address_pairs,
    "list-all-virtual-ip-address-address-pairs",
    "ListAllVirtualIPAddressAddressPairs",
    "List address pairs for the default page of virtual IP addresses",
    None,
    &[],
    []
);

pub(super) const LIST_INTERCONNECTS: Operation = paged!(
    ListInterconnectsInput,
    list_interconnects,
    "list-interconnects",
    "ListInterconnects",
    "List interconnects"
);

pub(super) const LIST_NETWORK_INTERFACES: Operation = paged!(
    ListNetworkInterfacesInput,
    list_network_interfaces,
    "list-network-interfaces",
    "ListNetworkInterfaces",
    "List elastic network interfaces"
);

pub(super) const LIST_PEERINGS: Operation = paged!(
    ListPeeringsInput,
    list_peerings,
    "list-peerings",
    "ListPeerings",
    "List VPC peerings"
);

pub(super) const LIST_ROUTE_TABLE_ROUTES: Operation = read!(
    ListRouteTableRoutesInput,
    sdk::ListRouteTableRoutesInput::default,
    list_route_table_routes,
    "list-route-table-routes",
    "ListRouteTableRoutes",
    "List routes on the default page of route tables",
    None,
    &[],
    []
);

pub(super) const LIST_ROUTE_TABLES: Operation = paged!(
    ListRouteTablesInput,
    list_route_tables,
    "list-route-tables",
    "ListRouteTables",
    "List route tables"
);

pub(super) const LIST_VIRTUAL_IP_ADDRESSES: Operation = paged!(
    ListVirtualIpAddressesInput,
    list_virtual_ip_addresses,
    "list-virtual-ip-addresses",
    "ListVirtualIPAddresses",
    "List virtual IP addresses"
);

pub(super) const LIST_VPN_CONNECTIONS: Operation = read!(
    ListVpnConnectionsInput, sdk::ListVpnConnectionsInput::default, list_vpn_connections,
    "list-vpn-connections", "ListVPNConnections", "List one page of VPN connections in hcm-3 and han-1",
    Some(concat!(
        "greennode network list-vpn-connections --page 1 --size 10 \\\n",
        "  --query \"Items[].{ID:UUID,Name:VPNName,Sites:length(VPNSites),\\\n",
        "Status:Status,Tunnels:length(VPNSites[].Tunnels[])}\" --output table"
    )),
    &[
        "Reads one page in `hcm-3` or `han-1`. Read-only profiles can run this command. `--page` defaults to 1 and `--size` defaults to 10. No automatic paging occurs. Live multi-page behavior remains unverified. Sites and tunnels are inline.",
        "Table and text output use one row per VPN with compact JSON for nested fields. The example selects site and tunnel counts. Status describes provisioning. Pre-shared keys are omitted. Server error messages and codes are withheld.",
    ],
    [PAGE => int_field!("Page", PAGE, page), SIZE => int_field!("Size", SIZE, size)]
);

pub(super) const LIST_WAN_IPS: Operation = paged!(
    ListWanIpsInput,
    list_wan_ips,
    "list-wanips",
    "ListWANIPs",
    "List WAN IP addresses"
);
