use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use greennode_sdk::cdn::{
    Client, DeleteWebAcceleratorInput, DisableWebAcceleratorInput, EnableWebAcceleratorInput,
    PurgePathsInput, UpdateWebAcceleratorInput,
};

use super::check_input;
use crate::cli::dispatch;
use crate::cli::input::{InputDescriptor, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};

struct Input {
    update: UpdateWebAcceleratorInput,
    cdn_domain: String,
    paths: Vec<String>,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            update: UpdateWebAcceleratorInput::new(""),
            cdn_domain: String::new(),
            paths: Vec::new(),
        }
    }
}

const fn field(name: &'static str, go_type: &'static str, required: bool) -> Field {
    Field {
        name,
        go_type,
        required,
    }
}

const CDN_ID: Field = field("cdn-id", "string", true);
const CDN_DOMAIN: Field = field("cdn-domain", "string", true);
const NO_WAIT: Field = field("no-wait", "bool", false);
const LB_TYPE: Field = field("lb-type", "*string", false);
const CERTIFICATE_ID: Field = field("certificate-id", "*string", false);
const ORIGIN_HOST_HEADER: Field = field("origin-host-header", "*string", false);
const SET_RULE_ACTIONS: Field = field("SetRuleActions", "[]cdn.RuleActionInput", false);
const REMOVE_RULE_ACTIONS: Field = field("RemoveRuleActions", "[]string", false);
const UPSTREAMS: Field = field("Upstreams", "[]cdn.UpstreamInput", false);
const FAIL_OVER_ERROR_CODES: Field = field("FailOverErrorCodes", "[]string", false);
const C_NAMES: Field = field("CNames", "[]string", false);
const PATHS: Field = field("Paths", "[]string", true);

macro_rules! bind {
    ($name:expr, $flag:expr, $requirement:expr, $($key:ident).+, $setter:expr) => {
        InputField {
            json_name: $name,
            flag: $flag,
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$($key).+ = serde_json::from_value(value).map_err(|_| ())?;
                Ok(())
            },
            set_flag: $setter,
            requirement: $requirement,
        }
    };
}

macro_rules! required_string {
    ($name:literal, $flag:ident, $($key:ident).+) => {
        bind!($name, Some($flag), Requirement::Nonzero(|input| !input.$($key).+.is_empty()), $($key).+, Some(|input, matches| {
            input.$($key).+ = matches.get_one::<String>($flag.name).cloned().ok_or(())?;
            Ok(())
        }))
    };
}

macro_rules! optional_string {
    ($name:literal, $flag:ident, $member:ident) => {
        bind!(
            $name,
            Some($flag),
            Requirement::Optional,
            update.$member,
            Some(|input, matches| {
                input.update.$member = matches.get_one::<String>($flag.name).cloned();
                Ok(())
            })
        )
    };
}

const ID_INPUT: InputField<Input> = required_string!("CDNID", CDN_ID, update.cdn_id);
const NO_WAIT_INPUT: InputField<Input> = bind!(
    "NoWait",
    Some(NO_WAIT),
    Requirement::Optional,
    update.no_wait,
    Some(|input, matches| {
        input.update.no_wait = *matches.get_one::<bool>(NO_WAIT.name).ok_or(())?;
        Ok(())
    })
);

const fn descriptor(fields: &'static [InputField<Input>]) -> InputDescriptor<Input> {
    InputDescriptor {
        defaults: Input::default,
        fields,
        allow_json: true,
        allow_query: true,
        project_binding: None,
        pre_guard: Some(check_input),
        guard: None,
    }
}

const DELETE_INPUT: InputDescriptor<Input> = descriptor(&[ID_INPUT]);
const TOGGLE_INPUT: InputDescriptor<Input> = descriptor(&[ID_INPUT, NO_WAIT_INPUT]);
const PURGE_INPUT: InputDescriptor<Input> = descriptor(&[
    required_string!("CDNDomain", CDN_DOMAIN, cdn_domain),
    bind!(
        PATHS.name,
        None,
        Requirement::Nonzero(|input| !input.paths.is_empty()),
        paths,
        None
    ),
]);
fn nested_fields<T: DeserializeOwned>(value: Value, names: &[&str]) -> Result<Vec<T>, ()> {
    let rows: Vec<Map<String, Value>> = serde_json::from_value(value).map_err(|_| ())?;
    rows.into_iter()
        .map(|row| {
            let mut normalized = Map::new();
            for (key, value) in row {
                let name = names
                    .iter()
                    .find(|name| {
                        key.chars()
                            .map(fold_field_char)
                            .eq(name.chars().map(fold_field_char))
                    })
                    .ok_or(())?;
                if normalized.insert((*name).into(), value).is_some() {
                    return Err(());
                }
            }
            serde_json::from_value(Value::Object(normalized)).map_err(|_| ())
        })
        .collect()
}

fn fold_field_char(character: char) -> char {
    match character {
        '\u{017f}' => 's',
        '\u{212a}' => 'k',
        _ => character.to_ascii_lowercase(),
    }
}

const UPDATE_INPUT: InputDescriptor<Input> = descriptor(&[
    ID_INPUT,
    InputField {
        set_json: |input, value| {
            input.update.set_rule_actions = nested_fields(value, &["Name", "Value"])?;
            Ok(())
        },
        ..bind!(
            SET_RULE_ACTIONS.name,
            None,
            Requirement::Optional,
            update.set_rule_actions,
            None
        )
    },
    bind!(
        REMOVE_RULE_ACTIONS.name,
        None,
        Requirement::Optional,
        update.remove_rule_actions,
        None
    ),
    InputField {
        set_json: |input, value| {
            input.update.upstreams = nested_fields(
                value,
                &[
                    "ID",
                    "Priority",
                    "IPAddress",
                    "UpstreamType",
                    "OriginValue",
                    "UseSSL",
                ],
            )?;
            Ok(())
        },
        ..bind!(
            UPSTREAMS.name,
            None,
            Requirement::Optional,
            update.upstreams,
            None
        )
    },
    optional_string!("LBType", LB_TYPE, lb_type),
    bind!(
        FAIL_OVER_ERROR_CODES.name,
        None,
        Requirement::Optional,
        update.fail_over_error_codes,
        None
    ),
    optional_string!("CertificateID", CERTIFICATE_ID, certificate_id),
    optional_string!("OriginHostHeader", ORIGIN_HOST_HEADER, origin_host_header),
    bind!(
        C_NAMES.name,
        None,
        Requirement::Optional,
        update.c_names,
        None
    ),
    NO_WAIT_INPUT,
]);

macro_rules! write {
    ($name:literal, $sdk:literal, $help:literal, $kind:ident, $method:ident, $input:ident, $value:expr, $flags:expr, $json:expr, $notes:expr, $example:expr) => {
        Operation {
            name: $name,
            sdk_operation: $sdk,
            short_help: $help,
            example: Some($example),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::$kind,
            flags: $flags,
            json_fields: $json,
            extra_flags: &[],
            notes: $notes,
            handler: Some(Handler::new(&$input, |operation, matches, invocation| {
                Box::pin(dispatch::management(
                    invocation,
                    operation,
                    matches,
                    &$input,
                    |ctx, config, input| {
                        Box::pin(async move {
                            let value = ($value)(input);
                            let result = Client::new(config).$method(ctx, value).await;
                            dispatch::project_result(result, std::convert::identity)
                        })
                    },
                ))
            })),
        }
    };
}

const TOGGLE_NOTES: &[&str] = &[
    "Verifies the CDN ID and status before writing. Confirms the toggle with reads at 0, 2, 4, and 8 seconds.",
    "Use `--no-wait` to skip settlement after confirmation. If the CDN does not settle, stdout holds the last safe detail and the command exits 1.",
];

pub(super) const UPDATE: Operation = write!(
    "update-web-accelerator",
    "cdn.UpdateWebAccelerator",
    "Update a Web Accelerator",
    Write,
    update_web_accelerator,
    UPDATE_INPUT,
    |input: &Input| input.update.clone(),
    &[CDN_ID, LB_TYPE, CERTIFICATE_ID, ORIGIN_HOST_HEADER, NO_WAIT],
    &[
        SET_RULE_ACTIONS,
        REMOVE_RULE_ACTIONS,
        UPSTREAMS,
        FAIL_OVER_ERROR_CODES,
        C_NAMES
    ],
    &[
        "Merges changes into a fresh detail with an explicit status and a matching CDN ID. Supply lists through `--cli-input-json`.",
        "Use `--no-wait` to return after one follow-up read."
    ],
    "greennode cdn update-web-accelerator --cdn-id <cdn-id> --cli-input-json file://changes.json"
);

pub(super) const DELETE: Operation = write!(
    "delete-web-accelerator",
    "cdn.DeleteWebAccelerator",
    "Delete a Web Accelerator",
    DestructiveWrite,
    delete_web_accelerator,
    DELETE_INPUT,
    |input: &Input| DeleteWebAcceleratorInput::new(&input.update.cdn_id),
    &[CDN_ID],
    &[],
    &[
        "Deletes the CDN and its generated CNAME target after verifying the CDN ID and status. Pass `--yes` to confirm."
    ],
    "greennode cdn delete-web-accelerator --cdn-id <cdn-id> --yes"
);

pub(super) const ENABLE: Operation = write!(
    "enable-web-accelerator",
    "cdn.EnableWebAccelerator",
    "Enable a Web Accelerator",
    Write,
    enable_web_accelerator,
    TOGGLE_INPUT,
    |input: &Input| EnableWebAcceleratorInput {
        cdn_id: input.update.cdn_id.clone(),
        no_wait: input.update.no_wait,
    },
    &[CDN_ID, NO_WAIT],
    &[],
    TOGGLE_NOTES,
    "greennode cdn enable-web-accelerator --cdn-id <cdn-id>"
);

pub(super) const DISABLE: Operation = write!(
    "disable-web-accelerator",
    "cdn.DisableWebAccelerator",
    "Disable a Web Accelerator",
    DestructiveWrite,
    disable_web_accelerator,
    TOGGLE_INPUT,
    |input: &Input| DisableWebAcceleratorInput {
        cdn_id: input.update.cdn_id.clone(),
        no_wait: input.update.no_wait,
    },
    &[CDN_ID, NO_WAIT],
    &[],
    TOGGLE_NOTES,
    "greennode cdn disable-web-accelerator --cdn-id <cdn-id> --yes"
);

pub(super) const PURGE: Operation = write!(
    "purge-paths",
    "cdn.PurgePaths",
    "Purge exact cached CDN paths",
    Write,
    purge_paths,
    PURGE_INPUT,
    |input: &Input| PurgePathsInput::new(&input.cdn_domain, input.paths.clone()),
    &[CDN_DOMAIN],
    &[PATHS],
    &[
        "Supply `Paths` through `--cli-input-json`. Wait at least 30 seconds between purges of a CDN. Each purge counts against the package's daily limit."
    ],
    "greennode cdn purge-paths --cdn-domain <cdn-domain> --cli-input-json '{\"Paths\":[\"/<path>\"]}'"
);
