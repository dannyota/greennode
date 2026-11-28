use clap::ArgMatches;
use greennode_sdk::{Config, Error, RequestContext, network as sdk};

use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::errors::{Rule, RuleExit, RuleName, RuleOperation};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};

use super::check_file;

macro_rules! field {
    ($json:literal, $flag:expr, $field:ident, $requirement:expr, $json_value:expr, $flag_value:expr) => {
        InputField {
            json_name: $json,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$field = ($json_value)(value)?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$field = ($flag_value)(matches, $flag.name)?;
                Ok(())
            }),
            requirement: $requirement,
        }
    };
}

macro_rules! op {
    ($command:literal, $sdk:literal, $method:ident, $type:ty, $kind:ident, $help:literal, $example:literal,
     $notes:expr, $guard:expr, $presence:expr, $defaults:expr, $run:ident, [$($flag:ident => $descriptor:expr),* $(,)?]) => {{
        const INPUT: InputDescriptor<$type> = InputDescriptor {
            defaults: $defaults,
            fields: &[$($descriptor),*],
            allow_json: true,
            allow_query: true,
            project_binding: None,
            pre_guard: Some(check_file),
            guard: $guard,
        };
        Operation {
            name: $command,
            sdk_operation: $sdk,
            short_help: $help,
            example: Some($example),
            presence_required_fields: $presence,
            inherited_bindings: &[],
            kind: Kind::$kind,
            flags: &[$($flag),*],
            json_fields: &[],
            extra_flags: &[],
            notes: $notes,
            handler: Some(Handler::new(&INPUT, |operation, matches, invocation| {
                Box::pin(dispatch::management(invocation, operation, matches, &INPUT, $run))
            })),
        }
    }};
}

fn string_json(value: serde_json::Value) -> Result<String, ()> {
    if let serde_json::Value::String(value) = value {
        Ok(value)
    } else {
        Err(())
    }
}

fn int_json(value: serde_json::Value) -> Result<i64, ()> {
    value.as_i64().ok_or(())
}

fn bool_json(value: serde_json::Value) -> Result<bool, ()> {
    value.as_bool().ok_or(())
}

fn string_flag(matches: &ArgMatches, name: &str) -> Result<String, ()> {
    matches.get_one::<String>(name).cloned().ok_or(())
}

fn int_flag(matches: &ArgMatches, name: &str) -> Result<i64, ()> {
    matches.get_one::<i64>(name).copied().ok_or(())
}

fn bool_flag(matches: &ArgMatches, name: &str) -> Result<bool, ()> {
    matches.get_one::<bool>(name).copied().ok_or(())
}

fn list_flag(matches: &ArgMatches, name: &str) -> Result<Vec<String>, ()> {
    matches
        .get_many::<String>(name)
        .map(|values| values.cloned().collect())
        .ok_or(())
}

fn confirm(matches: &ArgMatches, message: &'static str) -> Result<(), InputError> {
    if matches.get_one::<bool>("yes").copied().unwrap_or(false) {
        Ok(())
    } else {
        Err(InputError(message))
    }
}

const fn rule(
    identity: &'static str,
    rank: u16,
    code: &'static str,
    matches: fn(&Error) -> bool,
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
        exit: Some(RuleExit::Failure),
        message: None,
    }
}

pub(super) const UNEXPECTEDSTATUS: Rule = rule(
    "network.UnexpectedStatus",
    12,
    "UnexpectedStatus",
    |error| {
        error.detail::<sdk::NetworkWriteError>() == Some(&sdk::NetworkWriteError::UnexpectedStatus)
    },
);
pub(super) const DEFAULTRESOURCE: Rule =
    rule("network.DefaultResource", 31, "DefaultResource", |error| {
        error.detail::<sdk::NetworkWriteError>() == Some(&sdk::NetworkWriteError::DefaultResource)
    });
pub(super) const RESOURCEBUSY: Rule = rule("network.Busy", 32, "ResourceBusy", |error| {
    error.detail::<sdk::NetworkWriteError>() == Some(&sdk::NetworkWriteError::Busy)
});
const NAME: Field = Field {
    name: "name",
    go_type: "string",
    required: true,
};
const CIDR: Field = Field {
    name: "cidr",
    go_type: "string",
    required: true,
};
const NO_WAIT: Field = Field {
    name: "no-wait",
    go_type: "bool",
    required: false,
};
const VPC_ID: Field = Field {
    name: "vpc-id",
    go_type: "string",
    required: true,
};
const DNS_SERVERS: Field = Field {
    name: "dns-servers",
    go_type: "[]string",
    required: true,
};
const MTU: Field = Field {
    name: "mtu",
    go_type: "*int",
    required: false,
};
const DHCP_OPTIONS_ID: Field = Field {
    name: "dhcp-options-id",
    go_type: "string",
    required: true,
};
const ZONE_ID: Field = Field {
    name: "zone-id",
    go_type: "string",
    required: true,
};
const SUBNET_ID: Field = Field {
    name: "subnet-id",
    go_type: "string",
    required: true,
};
const ROUTE_TABLE_ID: Field = Field {
    name: "route-table-id",
    go_type: "string",
    required: true,
};
const DESTINATION_CIDR: Field = Field {
    name: "destination-cidr",
    go_type: "string",
    required: true,
};
const TARGET: Field = Field {
    name: "target",
    go_type: "string",
    required: true,
};
const NETWORK_ACL_ID: Field = Field {
    name: "network-acl-id",
    go_type: "string",
    required: true,
};
const DIRECTION: Field = Field {
    name: "direction",
    go_type: "string",
    required: true,
};
const PRIORITY_REQUIRED: Field = Field {
    name: "priority",
    go_type: "int",
    required: true,
};
const PROTOCOL: Field = Field {
    name: "protocol",
    go_type: "string",
    required: true,
};
const ACTION: Field = Field {
    name: "action",
    go_type: "string",
    required: true,
};
const PORT_RANGE_MIN: Field = Field {
    name: "port-range-min",
    go_type: "int",
    required: false,
};
const PORT_RANGE_MAX: Field = Field {
    name: "port-range-max",
    go_type: "int",
    required: false,
};
pub(super) mod acls;
pub(super) mod dhcp;
pub(super) mod routes;
pub(super) mod subnets;
pub(super) mod vpcs;
