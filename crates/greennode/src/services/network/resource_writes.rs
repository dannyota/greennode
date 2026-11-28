use std::net::IpAddr;

use greennode_sdk::network::*;

use super::*;
use crate::cli::errors::{Rule, RuleName, RuleOperation};

pub(super) const SYSTEM_GROUP: Rule =
    rule("network-system-group", "SystemSecurityGroup", 27, |error| {
        error.detail::<SystemSecurityGroup>().is_some()
    });
pub(super) const GROUP_IN_USE: Rule = rule(
    "network-security-group-in-use",
    "SecurityGroupInUse",
    28,
    |error| error.detail::<SecurityGroupInUse>().is_some(),
);
pub(super) const RESOURCE_IN_USE: Rule =
    rule("network-resource-in-use", "ResourceInUse", 30, |error| {
        error.detail::<ResourceInUse>().is_some()
    });

const fn rule(
    identity: &'static str,
    code: &'static str,
    rank: u16,
    matches: fn(&greennode_sdk::Error) -> bool,
) -> Rule {
    Rule {
        identity,
        matches,
        name: Some(RuleName {
            rank,
            code,
            operation: RuleOperation::Omit,
            include_api_status: false,
        }),
        exit: None,
        message: None,
    }
}

const fn flag(name: &'static str, go_type: &'static str, required: bool) -> Field {
    Field {
        name,
        go_type,
        required,
    }
}

const MISSING_PRICE: f64 = f64::from_bits(0x7ff8_0000_0000_42a1);

macro_rules! text {
    ($name:literal, $flag:ident, $field:ident, required) => {
        nat_field!(
            $name,
            $flag,
            $field,
            Requirement::Nonzero(|input| !input.$field.is_empty())
        )
    };
    ($name:literal, $flag:ident, $field:ident, optional) => {
        nat_field!($name, $flag, $field, Requirement::Optional)
    };
    ($name:literal, $flag:ident, $field:ident, pointer) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$field = Some(value.as_str().ok_or(())?.to_owned());
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$field = Some(matches.get_one::<String>($flag.name).ok_or(())?.clone());
                Ok(())
            }),
            requirement: Requirement::Optional,
        }
    };
}

macro_rules! boolean {
    ($name:literal, $flag:ident, $field:ident) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$field = value.as_bool().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$field = *matches.get_one::<bool>($flag.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Optional,
        }
    };
}

macro_rules! max_price {
    ($flag:ident) => {
        InputField {
            json_name: "MaxPrice",
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.max_price = value.as_f64().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.max_price = *matches.get_one::<f64>($flag.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Presence,
        }
    };
}

macro_rules! operation {
    ($command:literal, $sdk:literal, $kind:ident, $help:literal, $example:literal, $method:ident, $descriptor:ident, $flags:expr, $presence:expr, $notes:expr) => {
        Operation {
            name: $command,
            sdk_operation: $sdk,
            kind: Kind::$kind,
            short_help: $help,
            example: Some($example),
            flags: $flags,
            notes: $notes,
            presence_required_fields: $presence,
            inherited_bindings: &[],
            json_fields: &[],
            extra_flags: &[],
            handler: Some(Handler::new(
                &$descriptor,
                |operation, matches, invocation| {
                    Box::pin(dispatch::management(
                        invocation,
                        operation,
                        matches,
                        &$descriptor,
                        $method,
                    ))
                },
            )),
        }
    };
}

fn world_open(
    input: &CreateSecurityGroupRuleInput,
    matches: &ArgMatches,
) -> Result<(), InputError> {
    if matches.get_one::<bool>("yes") == Some(&true)
        || input.direction.eq_ignore_ascii_case("egress")
    {
        return Ok(());
    }
    if let Some((address, "0")) = input.remote_ip_prefix.split_once('/')
        && address.parse::<IpAddr>().is_ok()
    {
        return Err(InputError(
            "create-security-group-rule: a rule that is not egress with prefix length 0 is open to the entire internet; pass --yes to confirm",
        ));
    }
    Ok(())
}

fn endpoint_selectors(matches: &ArgMatches, json: Option<&str>) -> Result<(), InputError> {
    for (flag, message) in [
        (
            "availability-zone-id",
            "--availability-zone-id must not be empty",
        ),
        ("category-id", "--category-id must not be empty"),
        ("service-id", "--service-id must not be empty"),
        ("package-id", "--package-id must not be empty"),
    ] {
        if matches
            .get_one::<String>(flag)
            .is_some_and(String::is_empty)
        {
            return Err(InputError(message));
        }
    }
    check_file(matches, json)
}

fn purchase_consent(
    matches: &ArgMatches,
    price: f64,
    message: &'static str,
) -> Result<(), InputError> {
    if matches.get_one::<bool>("yes") != Some(&true) {
        return Err(InputError(message));
    }
    if price.to_bits() == MISSING_PRICE.to_bits() {
        return Err(InputError(
            "--max-price is required; JSON MaxPrice must be non-null",
        ));
    }
    if !price.is_finite() || price < 0.0 {
        return Err(InputError("--max-price must be finite and nonnegative"));
    }
    Ok(())
}

const NAME_REQUIRED: Field = flag("name", "string", true);
const DESCRIPTION: Field = flag("description", "string", false);
const NO_WAIT: Field = flag("no-wait", "bool", false);
const SECURITY_GROUP_ID_REQUIRED: Field = flag("security-group-id", "string", true);
const NAME_POINTER: Field = flag("name", "*string", false);
const DESCRIPTION_POINTER: Field = flag("description", "*string", false);
const DIRECTION_REQUIRED: Field = flag("direction", "string", true);
const PROTOCOL_REQUIRED: Field = flag("protocol", "string", true);
const REMOTE_IP_PREFIX_REQUIRED: Field = flag("remote-ip-prefix", "string", true);
const ETHER_TYPE: Field = flag("ether-type", "string", false);
const PORT_RANGE_MIN: Field = flag("port-range-min", "int", false);
const PORT_RANGE_MAX: Field = flag("port-range-max", "int", false);
const SECURITY_GROUP_RULE_ID_REQUIRED: Field = flag("security-group-rule-id", "string", true);
const SUBNET_ID_REQUIRED: Field = flag("subnet-id", "string", true);
const MODE_REQUIRED: Field = flag("mode", "string", true);
const IP_ADDRESS: Field = flag("ip-address", "string", false);
const VIRTUAL_IP_ADDRESS_ID_REQUIRED: Field = flag("virtual-ip-address-id", "string", true);
const MODE_POINTER: Field = flag("mode", "*string", false);
const VPC_ID_REQUIRED: Field = flag("vpc-id", "string", true);
const AVAILABILITY_ZONE_ID: Field = flag("availability-zone-id", "string", false);
const CATEGORY_ID: Field = flag("category-id", "string", false);
const SERVICE_ID: Field = flag("service-id", "string", false);
const PACKAGE_ID: Field = flag("package-id", "string", false);
const MAX_PRICE_REQUIRED: Field = flag("max-price", "*float64", true);
const ENDPOINT_ID_REQUIRED: Field = flag("endpoint-id", "string", true);
const ZONE_ID_REQUIRED: Field = flag("zone-id", "string", true);
const NAT_ID_REQUIRED: Field = flag("nat-id", "string", true);

const CREATE_SECURITY_GROUP_INPUT: InputDescriptor<CreateSecurityGroupInput> = InputDescriptor {
    defaults: || CreateSecurityGroupInput::new(""),
    fields: &[
        text!("Name", NAME_REQUIRED, name, required),
        text!("Description", DESCRIPTION, description, optional),
        boolean!("NoWait", NO_WAIT, no_wait),
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_file),
    guard: None,
};
run!(create_security_group, CreateSecurityGroupInput);
pub(super) const CREATE_SECURITY_GROUP: Operation = operation!(
    "create-security-group",
    "network.CreateSecurityGroup",
    Write,
    "Create security group",
    "greennode network create-security-group --name <name>",
    create_security_group,
    CREATE_SECURITY_GROUP_INPUT,
    &[NAME_REQUIRED, DESCRIPTION, NO_WAIT],
    &[],
    &[
        "Confirmed live: the new group is already ACTIVE in the create response itself, so the wait this command runs afterward settles on its first read. A duplicate --name fails with the server's own message at status 400."
    ]
);

const UPDATE_SECURITY_GROUP_INPUT: InputDescriptor<UpdateSecurityGroupInput> = InputDescriptor {
    defaults: || UpdateSecurityGroupInput::new(""),
    fields: &[
        text!(
            "SecurityGroupID",
            SECURITY_GROUP_ID_REQUIRED,
            security_group_id,
            required
        ),
        text!("Name", NAME_POINTER, name, pointer),
        text!("Description", DESCRIPTION_POINTER, description, pointer),
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_file),
    guard: None,
};
run!(update_security_group, UpdateSecurityGroupInput);
pub(super) const UPDATE_SECURITY_GROUP: Operation = operation!(
    "update-security-group",
    "network.UpdateSecurityGroup",
    Write,
    "Update security group",
    "greennode network update-security-group --security-group-id <security-group-id>",
    update_security_group,
    UPDATE_SECURITY_GROUP_INPUT,
    &[
        SECURITY_GROUP_ID_REQUIRED,
        NAME_POINTER,
        DESCRIPTION_POINTER
    ],
    &[],
    &[]
);

const DELETE_SECURITY_GROUP_INPUT: InputDescriptor<DeleteSecurityGroupInput> = InputDescriptor {
    defaults: || DeleteSecurityGroupInput::new(""),
    fields: &[text!(
        "SecurityGroupID",
        SECURITY_GROUP_ID_REQUIRED,
        security_group_id,
        required
    )],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_file),
    guard: None,
};
run!(delete_security_group, DeleteSecurityGroupInput);
pub(super) const DELETE_SECURITY_GROUP: Operation = operation!(
    "delete-security-group",
    "network.DeleteSecurityGroup",
    DestructiveWrite,
    "Delete security group",
    "greennode network delete-security-group --security-group-id <security-group-id> --yes",
    delete_security_group,
    DELETE_SECURITY_GROUP_INPUT,
    &[SECURITY_GROUP_ID_REQUIRED],
    &[],
    &[
        "Refuses, before any write, a system group or a group with any server attached. A repeat delete of an already-deleted group returns NotFound. The status of a delete the server itself refuses as in use for some other reason has not been confirmed live."
    ]
);

const CREATE_SECURITY_GROUP_RULE_INPUT: InputDescriptor<CreateSecurityGroupRuleInput> =
    InputDescriptor {
        defaults: || CreateSecurityGroupRuleInput::new("", "", "", ""),
        fields: &[
            text!(
                "SecurityGroupID",
                SECURITY_GROUP_ID_REQUIRED,
                security_group_id,
                required
            ),
            text!("Direction", DIRECTION_REQUIRED, direction, required),
            text!("Protocol", PROTOCOL_REQUIRED, protocol, required),
            text!(
                "RemoteIPPrefix",
                REMOTE_IP_PREFIX_REQUIRED,
                remote_ip_prefix,
                required
            ),
            text!("EtherType", ETHER_TYPE, ether_type, optional),
            integer!("PortRangeMin", PORT_RANGE_MIN, port_range_min),
            integer!("PortRangeMax", PORT_RANGE_MAX, port_range_max),
            text!("Description", DESCRIPTION, description, optional),
        ],
        allow_json: true,
        allow_query: true,
        project_binding: None,
        pre_guard: Some(check_file),
        guard: Some(world_open),
    };
run!(create_security_group_rule, CreateSecurityGroupRuleInput);
pub(super) const CREATE_SECURITY_GROUP_RULE: Operation = operation!(
    "create-security-group-rule",
    "network.CreateSecurityGroupRule",
    Write,
    "Create security group rule",
    "greennode network create-security-group-rule --security-group-id <security-group-id> --direction <direction> --protocol <protocol> --remote-ip-prefix <remote-ip-prefix>",
    create_security_group_rule,
    CREATE_SECURITY_GROUP_RULE_INPUT,
    &[
        SECURITY_GROUP_ID_REQUIRED,
        DIRECTION_REQUIRED,
        PROTOCOL_REQUIRED,
        REMOTE_IP_PREFIX_REQUIRED,
        ETHER_TYPE,
        PORT_RANGE_MIN,
        PORT_RANGE_MAX,
        DESCRIPTION
    ],
    &[],
    &[
        "Confirmed live: the new rule is already ACTIVE in the create response, so this command takes no wait. A rule that exactly duplicates an existing one fails with status 409; a rule that overlaps an existing one without duplicating it fails with status 400. --remote-ip-prefix is stored exactly as sent, host bits included: 203.0.113.5/24 is not masked to 203.0.113.0/24. A rule whose --direction is not egress and whose --remote-ip-prefix has prefix length 0, such as 0.0.0.0/0 or ::/0, needs --yes: it opens every port the rule names to the entire internet."
    ]
);

const DELETE_SECURITY_GROUP_RULE_INPUT: InputDescriptor<DeleteSecurityGroupRuleInput> =
    InputDescriptor {
        defaults: || DeleteSecurityGroupRuleInput::new("", ""),
        fields: &[
            text!(
                "SecurityGroupID",
                SECURITY_GROUP_ID_REQUIRED,
                security_group_id,
                required
            ),
            text!(
                "SecurityGroupRuleID",
                SECURITY_GROUP_RULE_ID_REQUIRED,
                security_group_rule_id,
                required
            ),
        ],
        allow_json: true,
        allow_query: true,
        project_binding: None,
        pre_guard: Some(check_file),
        guard: None,
    };
run!(delete_security_group_rule, DeleteSecurityGroupRuleInput);
pub(super) const DELETE_SECURITY_GROUP_RULE: Operation = operation!(
    "delete-security-group-rule",
    "network.DeleteSecurityGroupRule",
    DestructiveWrite,
    "Delete security group rule",
    "greennode network delete-security-group-rule --security-group-id <security-group-id> --security-group-rule-id <security-group-rule-id> --yes",
    delete_security_group_rule,
    DELETE_SECURITY_GROUP_RULE_INPUT,
    &[SECURITY_GROUP_ID_REQUIRED, SECURITY_GROUP_RULE_ID_REQUIRED],
    &[],
    &[
        "Refuses, before any write, a rule that does not belong to the named group. A repeat delete of an already-deleted rule also returns NotFound."
    ]
);

const CREATE_VIRTUAL_IP_ADDRESS_INPUT: InputDescriptor<CreateVirtualIpAddressInput> =
    InputDescriptor {
        defaults: || CreateVirtualIpAddressInput::new("", "", ""),
        fields: &[
            text!("SubnetID", SUBNET_ID_REQUIRED, subnet_id, required),
            text!("Name", NAME_REQUIRED, name, required),
            text!("Mode", MODE_REQUIRED, mode, required),
            text!("IPAddress", IP_ADDRESS, ip_address, optional),
            text!("Description", DESCRIPTION, description, optional),
        ],
        allow_json: true,
        allow_query: true,
        project_binding: None,
        pre_guard: Some(check_file),
        guard: None,
    };
run!(create_virtual_ip_address, CreateVirtualIpAddressInput);
pub(super) const CREATE_VIRTUAL_IP_ADDRESS: Operation = operation!(
    "create-virtual-ip-address",
    "network.CreateVirtualIPAddress",
    Write,
    "Create virtual ip address",
    "greennode network create-virtual-ip-address --subnet-id <subnet-id> --name <name> --mode <mode>",
    create_virtual_ip_address,
    CREATE_VIRTUAL_IP_ADDRESS_INPUT,
    &[
        SUBNET_ID_REQUIRED,
        NAME_REQUIRED,
        MODE_REQUIRED,
        IP_ADDRESS,
        DESCRIPTION
    ],
    &[],
    &[
        "Never resent after a failure that may have already reached the server: list-virtual-ip-addresses and match --name exactly, or --ip-address if one was given, before creating again, rather than retrying blind. Waits up to 60 seconds for ACTIVE only when the create response itself is not already there; whether the server ever actually returns an intermediate status, so this wait ever runs at all, has not been confirmed live."
    ]
);

const UPDATE_VIRTUAL_IP_ADDRESS_INPUT: InputDescriptor<UpdateVirtualIpAddressInput> =
    InputDescriptor {
        defaults: || UpdateVirtualIpAddressInput::new(""),
        fields: &[
            text!(
                "VirtualIPAddressID",
                VIRTUAL_IP_ADDRESS_ID_REQUIRED,
                virtual_ip_address_id,
                required
            ),
            text!("Name", NAME_POINTER, name, pointer),
            text!("Description", DESCRIPTION_POINTER, description, pointer),
            text!("Mode", MODE_POINTER, mode, pointer),
        ],
        allow_json: true,
        allow_query: true,
        project_binding: None,
        pre_guard: Some(check_file),
        guard: None,
    };
run!(update_virtual_ip_address, UpdateVirtualIpAddressInput);
pub(super) const UPDATE_VIRTUAL_IP_ADDRESS: Operation = operation!(
    "update-virtual-ip-address",
    "network.UpdateVirtualIPAddress",
    Write,
    "Update virtual ip address",
    "greennode network update-virtual-ip-address --virtual-ip-address-id <virtual-ip-address-id> --name <name>",
    update_virtual_ip_address,
    UPDATE_VIRTUAL_IP_ADDRESS_INPUT,
    &[
        VIRTUAL_IP_ADDRESS_ID_REQUIRED,
        NAME_POINTER,
        DESCRIPTION_POINTER,
        MODE_POINTER
    ],
    &[],
    &[]
);

const DELETE_VIRTUAL_IP_ADDRESS_INPUT: InputDescriptor<DeleteVirtualIpAddressInput> =
    InputDescriptor {
        defaults: || DeleteVirtualIpAddressInput::new(""),
        fields: &[text!(
            "VirtualIPAddressID",
            VIRTUAL_IP_ADDRESS_ID_REQUIRED,
            virtual_ip_address_id,
            required
        )],
        allow_json: true,
        allow_query: true,
        project_binding: None,
        pre_guard: Some(check_file),
        guard: None,
    };
run!(delete_virtual_ip_address, DeleteVirtualIpAddressInput);
pub(super) const DELETE_VIRTUAL_IP_ADDRESS: Operation = operation!(
    "delete-virtual-ip-address",
    "network.DeleteVirtualIPAddress",
    DestructiveWrite,
    "Delete virtual ip address",
    "greennode network delete-virtual-ip-address --virtual-ip-address-id <virtual-ip-address-id> --yes",
    delete_virtual_ip_address,
    DELETE_VIRTUAL_IP_ADDRESS_INPUT,
    &[VIRTUAL_IP_ADDRESS_ID_REQUIRED],
    &[],
    &[
        "After its read and before any DELETE, refuses with error code InvalidUsage a virtual IP whose type is not \"private\" (a public virtual IP has its own delete call), and with error code ResourceInUse, one that still has an address pair, found either on its own read or by list-address-pairs-by-virtual-ip-address, since a pair binds the address to a server interface and deleting it would move traffic."
    ]
);

const CREATE_ENDPOINT_INPUT: InputDescriptor<CreateEndpointInput> = InputDescriptor {
    defaults: || {
        let mut input = CreateEndpointInput::new("", "", "");
        input.max_price = MISSING_PRICE;
        input
    },
    fields: &[
        text!("Name", NAME_REQUIRED, name, required),
        text!("VPCID", VPC_ID_REQUIRED, vpc_id, required),
        text!("SubnetID", SUBNET_ID_REQUIRED, subnet_id, required),
        text!(
            "AvailabilityZoneID",
            AVAILABILITY_ZONE_ID,
            availability_zone_id,
            optional
        ),
        text!("CategoryID", CATEGORY_ID, category_id, optional),
        text!("ServiceID", SERVICE_ID, service_id, optional),
        text!("PackageID", PACKAGE_ID, package_id, optional),
        max_price!(MAX_PRICE_REQUIRED),
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(endpoint_selectors),
    guard: Some(|input, matches| {
        purchase_consent(
            matches,
            input.max_price,
            "create-endpoint buys a paid endpoint; pass --yes to confirm",
        )
    }),
};
run!(create_endpoint, CreateEndpointInput);
pub(super) const CREATE_ENDPOINT: Operation = operation!(
    "create-endpoint",
    "network.CreateEndpoint",
    Write,
    "Create endpoint",
    "greennode network create-endpoint --region hcm-3 --name demo --vpc-id <net-id> --subnet-id <sub-id> --max-price <VND-per-month> --yes",
    create_endpoint,
    CREATE_ENDPOINT_INPUT,
    &[
        NAME_REQUIRED,
        VPC_ID_REQUIRED,
        SUBNET_ID_REQUIRED,
        AVAILABILITY_ZONE_ID,
        CATEGORY_ID,
        SERVICE_ID,
        PACKAGE_ID,
        MAX_PRICE_REQUIRED
    ],
    &["MaxPrice"],
    &[
        "Buys one Gateway endpoint with one request and waits for ACTIVE. The body sets isEnableAutoRenew false to request auto-renew off, and private DNS stays off; No endpoint read returns the renewal setting, and billing list-resources reported a created endpoint's RenewType as AUTO-RENEW despite that request. Check billing list-resources after create. Use a fresh, unique name for each create.",
        "Supports Gateway endpoints in hcm-3 on an existing VPC and subnet, with private DNS off. It needs SDK IAM-user login and default endpoints; anything else returns InvalidConfig (exit 2). An endpoint changes network reachability; it does not grant bucket access and does not prove S3 connectivity.",
        "Select catalog UUIDs with --availability-zone-id, --category-id, --service-id, and --package-id. Omitted selectors default to HCM03-1C, the sole default category, vstorage.hcm03, and Standard. The catalog and placement must offer the selection; lifecycle behavior for other offers remains unverified.",
        "Requires --yes and --max-price (VND per month, or non-null JSON MaxPrice) before any request. Read-only profiles refuse this command. The cap is compared to a fresh quote taken just before create; it is not a server price lock. Quote and debit can differ. Unpriced or PriceAboveMax stops before purchase. A name that already exists in the VPC is refused.",
        "Waits up to 15 minutes after the request, polling every 5 seconds; a shorter context deadline wins. There is no --no-wait, DNS, renewal, tag, or raw request option.",
        "NotSettled prints safe partial output on stdout, ignoring --query. After it, run network list-endpoints and network get-endpoint in the same scope, then inspect billing. Do not repeat create while the outcome is unresolved. See Network endpoints documentation."
    ]
);

const DELETE_ENDPOINT_INPUT: InputDescriptor<DeleteEndpointInput> = InputDescriptor {
    defaults: || DeleteEndpointInput::new("", ""),
    fields: &[
        text!("EndpointID", ENDPOINT_ID_REQUIRED, endpoint_id, required),
        text!("VPCID", VPC_ID_REQUIRED, vpc_id, required),
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_file),
    guard: None,
};
run!(delete_endpoint, DeleteEndpointInput);
pub(super) const DELETE_ENDPOINT: Operation = operation!(
    "delete-endpoint",
    "network.DeleteEndpoint",
    DestructiveWrite,
    "Delete endpoint",
    "greennode network delete-endpoint --region hcm-3 --endpoint-id <enp-id> --vpc-id <net-id> --yes",
    delete_endpoint,
    DELETE_ENDPOINT_INPUT,
    &[ENDPOINT_ID_REQUIRED, VPC_ID_REQUIRED],
    &[],
    &[
        "Requires --yes. Deleting an endpoint that a cluster uses breaks that cluster's private access. The command refuses an endpoint with an attached resource or domain, but those fields do not prove that no cluster uses it.",
        "Supports Gateway endpoints in hcm-3 on an existing VPC and subnet, with private DNS off. It needs SDK IAM-user login and default endpoints; anything else returns InvalidConfig (exit 2). An endpoint changes network reachability; it does not grant bucket access and does not prove S3 connectivity.",
        "Requires explicit --endpoint-id and --vpc-id, refuses read-only profiles before any request, and sends one delete for an ACTIVE endpoint of the named VPC. Waits up to 10 minutes, polling every 5 seconds, until a complete list omits the ID. There is no --no-wait or --force. Deletion promises no refund, and a refund can stay pending after delete. In one probe, an endpoint that reused a just-deleted endpoint's name was deleted but had not been refunded hours after delete, and billing still listed it as active. Check billing list-transactions for the refund after delete. NotFound exits 4. After NotSettled, run network list-endpoints in the same scope before another action. See Network endpoints documentation."
    ]
);

mod nat;
pub(super) use nat::{CREATE_NAT_INSTANCE, DELETE_NAT_INSTANCE};
