use std::net::Ipv4Addr;

use super::*;
use serde_json::Value;

const NAME_REQUIRED: Field = flag("name", "string", true);
const PROTOCOL: Field = flag("protocol", "string", true);
const PORT: Field = flag("port", "int", true);
const CIDRS_REQUIRED: Field = flag("allowed-cidrs", "string", true);
const CIDRS: Field = flag("allowed-cidrs", "string", false);

const CIDRS_REQUIRED_MESSAGE: &str =
    "--allowed-cidrs is required (or set AllowedCIDRs through --cli-input-json)";
const PUBLIC_CREATE_MESSAGE: &str = "create-listener: AllowedCIDRs has an entry outside every private range (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 100.64.0.0/10); pass --yes to confirm";
const PUBLIC_UPDATE_MESSAGE: &str = "update-listener: AllowedCIDRs has an entry outside every private range (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 100.64.0.0/10); pass --yes to confirm";

macro_rules! option_field {
    ($field:ident, $flag:ident, $name:literal, $ty:ty, $decode:expr) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input: &mut _, value| {
                input.$field = Some(($decode)(value).ok_or(())?);
                Ok(())
            },
            set_flag: Some(|input: &mut _, matches| {
                input.$field = Some(matches.get_one::<$ty>($flag.name).ok_or(())?.clone());
                Ok(())
            }),
            requirement: Requirement::Optional,
        }
    };
}

macro_rules! json_list {
    ($field:ident, $name:literal, $decode:ident, $into:expr) => {
        InputField {
            json_name: $name,
            flag: None,
            secret: false,
            nullable: false,
            set_json: |input: &mut _, value| {
                input.$field = ($into)($decode(value)?);
                Ok(())
            },
            set_flag: None,
            requirement: Requirement::Optional,
        }
    };
}

macro_rules! cidr_field {
    ($flag:ident, $into:expr) => {
        InputField {
            json_name: "AllowedCIDRs",
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input: &mut _, value| {
                input.allowed_cidrs = ($into)(strings(value)?);
                Ok(())
            },
            set_flag: Some(|input: &mut _, matches| {
                let text = matches.get_one::<String>($flag.name).ok_or(())?;
                input.allowed_cidrs = ($into)(split_cidrs(text));
                Ok(())
            }),
            requirement: Requirement::Optional,
        }
    };
}

macro_rules! listener_op {
    ($input:ident, $method:ident, $name:literal, $go:literal, $help:literal, $example:literal,
     $notes:expr, $guard:ident, [$($flag:ident => $field:expr),* $(,)?], [$($json_flag:expr => $json_field:expr),+ $(,)?]) => {{
        const INPUT: InputDescriptor<sdk::$input> = InputDescriptor {
            defaults: sdk::$input::default,
            fields: &[$($field,)* $($json_field,)*],
            allow_json: true,
            allow_query: true,
            project_binding: None,
            pre_guard: Some(check_input_source),
            guard: Some($guard),
        };
        Operation {
            name: $name,
            sdk_operation: concat!("loadbalancer.", $go),
            short_help: $help,
            example: Some($example),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Write,
            flags: &[$($flag),*],
            json_fields: &[$($json_flag),*],
            extra_flags: &[],
            notes: $notes,
            handler: Some(Handler::new(&INPUT, |operation, matches, invocation| {
                Box::pin(dispatch::management_with_view(
                    invocation, operation, matches, &INPUT,
                    |ctx, config, input| Box::pin(async move {
                        let result = sdk::Client::new(config).$method(ctx, input.clone()).await;
                        dispatch::project_result(result, std::convert::identity)
                    }),
                    OutputView::default(),
                ))
            })),
        }
    }};
}

fn strings(value: Value) -> Result<Vec<String>, ()> {
    serde_json::from_value(value).map_err(|_| ())
}

fn headers(value: Value) -> Result<Vec<sdk::ListenerInsertHeader>, ()> {
    let Value::Array(items) = value else {
        return Err(());
    };
    items
        .into_iter()
        .map(|item| {
            let Value::Object(object) = item else {
                return Err(());
            };
            let mut name = None;
            let mut value = None;
            for (key, item) in object {
                let field = if key.eq_ignore_ascii_case("HeaderName") {
                    &mut name
                } else if key.eq_ignore_ascii_case("HeaderValue") {
                    &mut value
                } else {
                    return Err(());
                };
                if field.is_some() {
                    return Err(());
                }
                *field = Some(if item.is_null() {
                    String::new()
                } else {
                    item.as_str().ok_or(())?.to_owned()
                });
            }
            Ok(sdk::ListenerInsertHeader {
                header_name: name.unwrap_or_default(),
                header_value: value.unwrap_or_default(),
            })
        })
        .collect()
}

fn split_cidrs(value: &str) -> Vec<String> {
    if value.is_empty() {
        Vec::new()
    } else {
        value
            .split(',')
            .map(|part| part.trim().to_owned())
            .collect()
    }
}

fn private_cidr(value: &str) -> bool {
    let Some((address, bits)) = value.split_once('/') else {
        return false;
    };
    let Ok(address) = address.parse::<Ipv4Addr>() else {
        return false;
    };
    let Ok(length) = bits.parse::<u8>() else {
        return false;
    };
    if length > 32 || bits.starts_with('+') || (bits.starts_with('0') && bits.len() > 1) {
        return false;
    }
    let address = u32::from(address);
    let private = [
        (0x0a000000, 8_u8),
        (0xac100000, 12),
        (0xc0a80000, 16),
        (0x64400000, 10),
    ];
    private.into_iter().any(|(network, bits)| {
        length >= bits && (address >> (32 - bits)) == (network >> (32 - bits))
    })
}

fn confirm_cidrs(
    matches: &ArgMatches,
    cidrs: &[String],
    message: &'static str,
) -> Result<(), InputError> {
    if matches.get_one::<bool>("yes").copied().unwrap_or(false)
        || cidrs.iter().all(|cidr| private_cidr(cidr))
    {
        Ok(())
    } else {
        Err(InputError(message))
    }
}

fn guard_create(input: &sdk::CreateListenerInput, matches: &ArgMatches) -> Result<(), InputError> {
    if input.allowed_cidrs.is_empty() {
        return Err(InputError(CIDRS_REQUIRED_MESSAGE));
    }
    confirm_cidrs(matches, &input.allowed_cidrs, PUBLIC_CREATE_MESSAGE)
}

fn guard_update(input: &sdk::UpdateListenerInput, matches: &ArgMatches) -> Result<(), InputError> {
    confirm_cidrs(
        matches,
        input.allowed_cidrs.as_deref().unwrap_or_default(),
        PUBLIC_UPDATE_MESSAGE,
    )
}

const DEFAULT_POOL_ID: Field = flag("default-pool-id", "string", false);
const UPDATE_DEFAULT_POOL_ID: Field = flag("default-pool-id", "*string", false);
const TIMEOUT_CLIENT: Field = flag("timeout-client", "int", false);
const UPDATE_TIMEOUT_CLIENT: Field = flag("timeout-client", "*int", false);
const TIMEOUT_MEMBER: Field = flag("timeout-member", "int", false);
const UPDATE_TIMEOUT_MEMBER: Field = flag("timeout-member", "*int", false);
const TIMEOUT_CONNECTION: Field = flag("timeout-connection", "int", false);
const UPDATE_TIMEOUT_CONNECTION: Field = flag("timeout-connection", "*int", false);
const DEFAULT_CERTIFICATE_ID: Field = flag("default-certificate-id", "string", false);
const UPDATE_DEFAULT_CERTIFICATE_ID: Field = flag("default-certificate-id", "*string", false);
const CLIENT_CERTIFICATE_ID: Field = flag("client-certificate-id", "string", false);
const UPDATE_CLIENT_CERTIFICATE_ID: Field = flag("client-certificate-id", "*string", false);

pub(super) const CREATE_LISTENER: Operation = listener_op!(
    CreateListenerInput, create_listener, "create-listener", "CreateListener", "Create a load balancer listener",
    "greennode loadbalancer create-listener --load-balancer-id <load-balancer-id> --name <name> --protocol HTTP --port <port> --allowed-cidrs 10.0.0.0/24",
    &["Supply a comma-separated list of IPv4 CIDR prefixes with no host bits set. Use `--yes` for any entry outside `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, or `100.64.0.0/10`. HTTPS requires a default certificate. Use `--no-wait` to return submitted settings."], guard_create, [
        LOAD_BALANCER_ID => required_id!(sdk::CreateListenerInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
        NAME_REQUIRED => required_id!(sdk::CreateListenerInput, name, NAME_REQUIRED, "Name"),
        PROTOCOL => required_id!(sdk::CreateListenerInput, protocol, PROTOCOL, "Protocol"),
        PORT => InputField { requirement: Requirement::Nonzero(|input: &sdk::CreateListenerInput| input.port != 0), ..int_field!(port, PORT, "Port") },
        DEFAULT_POOL_ID => string_field!(sdk::CreateListenerInput, default_pool_id, DEFAULT_POOL_ID, "DefaultPoolID", Requirement::Optional),
        TIMEOUT_CLIENT => int_field!(timeout_client, TIMEOUT_CLIENT, "TimeoutClient"),
        TIMEOUT_MEMBER => int_field!(timeout_member, TIMEOUT_MEMBER, "TimeoutMember"),
        TIMEOUT_CONNECTION => int_field!(timeout_connection, TIMEOUT_CONNECTION, "TimeoutConnection"),
        DEFAULT_CERTIFICATE_ID => string_field!(sdk::CreateListenerInput, default_certificate_id, DEFAULT_CERTIFICATE_ID, "DefaultCertificateID", Requirement::Optional),
        CLIENT_CERTIFICATE_ID => string_field!(sdk::CreateListenerInput, client_certificate_id, CLIENT_CERTIFICATE_ID, "ClientCertificateID", Requirement::Optional),
        NO_WAIT => optional_bool!(no_wait, NO_WAIT, "NoWait"),
        CIDRS_REQUIRED => cidr_field!(CIDRS_REQUIRED, std::convert::identity),
    ], [
        flag("CertificateIDs", "[]string", false) => json_list!(certificate_ids, "CertificateIDs", strings, std::convert::identity),
        flag("InsertHeaders", "[]loadbalancer.ListenerInsertHeader", false) => json_list!(insert_headers, "InsertHeaders", headers, std::convert::identity),
    ]
);

pub(super) const UPDATE_LISTENER: Operation = listener_op!(
    UpdateListenerInput, update_listener, "update-listener", "UpdateListener", "Update a load balancer listener",
    "greennode loadbalancer update-listener --load-balancer-id <load-balancer-id> --listener-id <listener-id> --timeout-client <timeout-client>",
    &["Reads and merges the listener’s current settings. Supply at least one setting. If you change `--allowed-cidrs`, use `--yes` for entries outside the private ranges. Use `--no-wait` to return submitted settings."], guard_update, [
        LOAD_BALANCER_ID => required_id!(sdk::UpdateListenerInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
        LISTENER_ID => required_id!(sdk::UpdateListenerInput, listener_id, LISTENER_ID, "ListenerID"),
        UPDATE_DEFAULT_POOL_ID => option_field!(default_pool_id, UPDATE_DEFAULT_POOL_ID, "DefaultPoolID", String, |value: Value| value.as_str().map(str::to_owned)),
        UPDATE_TIMEOUT_CLIENT => option_field!(timeout_client, UPDATE_TIMEOUT_CLIENT, "TimeoutClient", i64, |value: Value| value.as_i64()),
        UPDATE_TIMEOUT_MEMBER => option_field!(timeout_member, UPDATE_TIMEOUT_MEMBER, "TimeoutMember", i64, |value: Value| value.as_i64()),
        UPDATE_TIMEOUT_CONNECTION => option_field!(timeout_connection, UPDATE_TIMEOUT_CONNECTION, "TimeoutConnection", i64, |value: Value| value.as_i64()),
        UPDATE_DEFAULT_CERTIFICATE_ID => option_field!(default_certificate_id, UPDATE_DEFAULT_CERTIFICATE_ID, "DefaultCertificateID", String, |value: Value| value.as_str().map(str::to_owned)),
        UPDATE_CLIENT_CERTIFICATE_ID => option_field!(client_certificate_id, UPDATE_CLIENT_CERTIFICATE_ID, "ClientCertificateID", String, |value: Value| value.as_str().map(str::to_owned)),
        NO_WAIT => optional_bool!(no_wait, NO_WAIT, "NoWait"),
        CIDRS => cidr_field!(CIDRS, Some),
    ], [
        flag("CertificateIDs", "*[]string", false) => json_list!(certificate_ids, "CertificateIDs", strings, Some),
        flag("InsertHeaders", "*[]loadbalancer.ListenerInsertHeader", false) => json_list!(insert_headers, "InsertHeaders", headers, Some),
    ]
);
