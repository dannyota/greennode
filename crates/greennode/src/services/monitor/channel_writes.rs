use clap::{ArgMatches, parser::ValueSource};
use greennode_sdk::Secret;
use greennode_sdk::monitor as sdk;
use serde::Deserialize;
use serde_json::value::RawValue;
use std::collections::BTreeMap;

use crate::cli::dispatch::{self, OutputView};
use crate::cli::input::{
    InputDescriptor, InputError, InputField, InputGuard, JsonChecks, Requirement,
};
use crate::cli::op::{Field, Handler, Kind, Operation};

#[derive(Default)]
struct Input {
    channel_id: String,
    name: Option<String>,
    r#type: String,
    address: Option<String>,
    headers: Option<Vec<sdk::ChannelHeader>>,
    otp_ref: String,
    otp: zeroize::Zeroizing<String>,
}

const fn flag(name: &'static str, go_type: &'static str, required: bool) -> Field {
    Field {
        name,
        go_type,
        required,
    }
}
const CHANNEL_ID: Field = flag("channel-id", "string", true);
const NAME: Field = flag("name", "string", true);
const OPTIONAL_NAME: Field = flag("name", "*string", false);
const TYPE: Field = flag("type", "string", true);
const ADDRESS: Field = flag("address", "string", true);
const OPTIONAL_ADDRESS: Field = flag("address", "*string", false);
const HEADERS: Field = flag("Headers", "[]monitor.ChannelHeader", false);
const OPTIONAL_HEADERS: Field = flag("Headers", "*[]monitor.ChannelHeader", false);
const OTP_REF: Field = flag("otp-ref", "string", false);
const OTP: Field = flag("otp", "string", false);

macro_rules! text {
    ($json:literal,$flag:ident,$field:ident,$wrap:expr,$requirement:expr) => {
        InputField {
            json_name: $json,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                let text = serde_json::from_value::<String>(value).map_err(|_| ())?;
                input.$field = ($wrap)(text);
                Ok(())
            },
            set_flag: Some(|input, matches| {
                let text = matches.get_one::<String>($flag.name).cloned().ok_or(())?;
                input.$field = ($wrap)(text);
                Ok(())
            }),
            requirement: $requirement,
        }
    };
}
const CHANNEL_ID_INPUT: InputField<Input> = text!(
    "ChannelID",
    CHANNEL_ID,
    channel_id,
    std::convert::identity,
    Requirement::Nonzero(|input| !input.channel_id.is_empty())
);
const NAME_INPUT: InputField<Input> = text!(
    "Name",
    NAME,
    name,
    Some,
    Requirement::Nonzero(|input| input.name.as_deref().is_some_and(|value| !value.is_empty()))
);
const ADDRESS_INPUT: InputField<Input> = text!(
    "Address",
    ADDRESS,
    address,
    Some,
    Requirement::Nonzero(|input| input
        .address
        .as_deref()
        .is_some_and(|value| !value.is_empty()))
);
const OPTIONAL_NAME_INPUT: InputField<Input> =
    text!("Name", OPTIONAL_NAME, name, Some, Requirement::Optional);
const OPTIONAL_ADDRESS_INPUT: InputField<Input> = text!(
    "Address",
    OPTIONAL_ADDRESS,
    address,
    Some,
    Requirement::Optional
);
const TYPE_INPUT: InputField<Input> = text!(
    "Type",
    TYPE,
    r#type,
    std::convert::identity,
    Requirement::Nonzero(|input| !input.r#type.is_empty())
);
const OTP_REF_INPUT: InputField<Input> = text!(
    "OTPRef",
    OTP_REF,
    otp_ref,
    std::convert::identity,
    Requirement::Optional
);
const OTP_INPUT: InputField<Input> = text!(
    "OTP",
    OTP,
    otp,
    zeroize::Zeroizing::new,
    Requirement::Optional
);

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct Header {
    #[serde(alias = "key")]
    key: String,
    #[serde(alias = "value")]
    value: String,
}
const HEADERS_INPUT: InputField<Input> = InputField {
    json_name: "Headers",
    flag: None,
    secret: false,
    nullable: false,
    set_flag: None,
    requirement: Requirement::Optional,
    set_json: |input, value| {
        let headers = serde_json::from_value::<Vec<Header>>(value).map_err(|_| ())?;
        input.headers = Some(
            headers
                .into_iter()
                .map(|header| sdk::ChannelHeader {
                    key: header.key,
                    value: header.value,
                })
                .collect(),
        );
        Ok(())
    },
};

const fn descriptor(
    fields: &'static [InputField<Input>],
    guard: Option<InputGuard<Input>>,
) -> InputDescriptor<Input> {
    InputDescriptor {
        defaults: Input::default,
        fields,
        allow_json: true,
        allow_query: true,
        project_binding: None,
        pre_guard: None,
        guard,
    }
}
const CREATE_INPUT: InputDescriptor<Input> = descriptor(
    &[
        NAME_INPUT,
        TYPE_INPUT,
        ADDRESS_INPUT,
        HEADERS_INPUT,
        OTP_REF_INPUT,
        OTP_INPUT,
    ],
    Some(guard_creation),
);
const UPDATE_INPUT: InputDescriptor<Input> = descriptor(
    &[
        CHANNEL_ID_INPUT,
        OPTIONAL_NAME_INPUT,
        OPTIONAL_ADDRESS_INPUT,
        HEADERS_INPUT,
        OTP_REF_INPUT,
        OTP_INPUT,
    ],
    Some(guard_update),
);
const DELETE_INPUT: InputDescriptor<Input> = descriptor(&[CHANNEL_ID_INPUT], None);
const SEND_INPUT: InputDescriptor<Input> = descriptor(
    &[TYPE_INPUT, ADDRESS_INPUT, HEADERS_INPUT],
    Some(guard_creation),
);

fn literal_fields(matches: &ArgMatches) -> (bool, bool) {
    let flag = matches.value_source("address") == Some(ValueSource::CommandLine);
    let json = matches
        .try_get_one::<String>("cli-input-json")
        .ok()
        .flatten()
        .filter(|json| !json.starts_with("file://"));
    let fields =
        json.and_then(|json| serde_json::from_str::<BTreeMap<String, &RawValue>>(json).ok());
    fields.as_ref().map_or((flag, false), |fields| {
        (
            flag || fields.keys().any(|key| key.eq_ignore_ascii_case("address")),
            fields.keys().any(|key| key.eq_ignore_ascii_case("headers")),
        )
    })
}
const HEADERS_MESSAGE: &str = "inline --cli-input-json Headers can hold a secret; pass it only through --cli-input-json file://channel.json";
const ADDRESS_MESSAGE: &str =
    "--address can hold a secret; pass it only through --cli-input-json file://channel.json";
fn guard_creation(input: &Input, matches: &ArgMatches) -> Result<(), InputError> {
    let (address, headers) = literal_fields(matches);
    if address
        && !["Email", "SMS", "Telegram"]
            .iter()
            .any(|kind| kind.eq_ignore_ascii_case(&input.r#type))
    {
        return Err(InputError(match input.r#type.as_str() {
            "Slack" => {
                "--address for a Slack channel can hold a secret; pass it only through --cli-input-json file://channel.json"
            }
            "Webhook" => {
                "--address for a Webhook channel can hold a secret; pass it only through --cli-input-json file://channel.json"
            }
            _ => ADDRESS_MESSAGE,
        }));
    }
    if headers {
        return Err(InputError(HEADERS_MESSAGE));
    }
    Ok(())
}
fn guard_update(_: &Input, matches: &ArgMatches) -> Result<(), InputError> {
    let (address, headers) = literal_fields(matches);
    if address {
        return Err(InputError(ADDRESS_MESSAGE));
    }
    if headers {
        return Err(InputError(HEADERS_MESSAGE));
    }
    Ok(())
}

fn check_json(bytes: &[u8]) -> Result<(), InputError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase", deny_unknown_fields)]
    struct ChannelInput {
        #[serde(rename = "ChannelID")]
        _channel_id: Option<String>,
        #[serde(rename = "Name")]
        _name: Option<String>,
        #[serde(rename = "Type")]
        _type: Option<String>,
        #[serde(rename = "Address")]
        _address: Option<String>,
        #[serde(rename = "Headers")]
        _headers: Option<Vec<Header>>,
        #[serde(rename = "OTPRef")]
        _otp_ref: Option<String>,
        #[serde(rename = "OTP")]
        _otp: Option<String>,
    }
    serde_json::from_slice::<Option<ChannelInput>>(bytes)
        .map(|_| ())
        .map_err(|_| InputError("--cli-input-json: invalid channel input"))
}
const CHECKS: JsonChecks = JsonChecks {
    validate: check_json,
    read_error: |error| error,
};

macro_rules! write {
    ($name:literal,$sdk:literal,$title:literal,$method:ident,$descriptor:ident,$kind:ident,$flags:expr,$json:expr,$value:expr,$project:expr,$notes:expr,$example:literal) => {
        Operation {
            name: $name,
            sdk_operation: concat!("monitor.", $sdk),
            short_help: $title,
            example: Some(concat!("greennode monitor ", $example)),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::$kind,
            flags: $flags,
            json_fields: $json,
            extra_flags: &[],
            notes: $notes,
            handler: Some(Handler::new(
                &$descriptor,
                |operation, matches, invocation| {
                    Box::pin(dispatch::management_custom(
                        invocation,
                        operation,
                        matches,
                        &$descriptor,
                        |ctx, config, input| {
                            Box::pin(async move {
                                let result =
                                    sdk::Client::new(config).$method(ctx, ($value)(input)).await;
                                dispatch::project_result(result, $project)
                            })
                        },
                        OutputView::default(),
                        Some(CHECKS),
                    ))
                },
            )),
        }
    };
}

pub(super) const CREATE: Operation = write!(
    "create-channel",
    "CreateChannel",
    "Create a notification channel",
    create_channel,
    CREATE_INPUT,
    Write,
    &[NAME, TYPE, ADDRESS, OTP_REF, OTP],
    &[HEADERS],
    |input: &Input| {
        let mut value = sdk::CreateChannelInput::new(
            input.name.as_deref().unwrap_or_default(),
            &input.r#type,
            input.address.as_deref().unwrap_or_default(),
        );
        value.headers = input.headers.clone().unwrap_or_default();
        value.otp_ref.clone_from(&input.otp_ref);
        value.otp = Secret::new(input.otp.as_str());
        value
    },
    |mut value: sdk::CreateChannelOutput| {
        value.channel = value.channel.for_display();
        value
    },
    &[
        "Pass Slack and Webhook addresses and all Headers through --cli-input-json file://channel.json. Email, Slack, SMS, and Telegram can validate an OTP supplied with its reference. Webhook refuses OTP fields. Channel credentials are masked before rendering or querying."
    ],
    "create-channel --name <name> --type Webhook --cli-input-json file://channel.json"
);
pub(super) const UPDATE: Operation = write!(
    "update-channel",
    "UpdateChannel",
    "Update a notification channel",
    update_channel,
    UPDATE_INPUT,
    Write,
    &[CHANNEL_ID, OPTIONAL_NAME, OPTIONAL_ADDRESS, OTP_REF, OTP],
    &[OPTIONAL_HEADERS],
    |input: &Input| {
        let mut value = sdk::UpdateChannelInput::new(&input.channel_id);
        value.name.clone_from(&input.name);
        value.address.clone_from(&input.address);
        value.headers.clone_from(&input.headers);
        value.otp_ref.clone_from(&input.otp_ref);
        value.otp = Secret::new(input.otp.as_str());
        value
    },
    |mut value: sdk::UpdateChannelOutput| {
        value.channel = value.channel.for_display();
        value
    },
    &[
        "Pass Address and Headers through --cli-input-json file://channel.json. Omitted fields keep their current values. An empty Headers list clears the headers. An update that carries a validated OTP sends once."
    ],
    "update-channel --channel-id <channel-id> --cli-input-json file://channel.json"
);
pub(super) const DELETE: Operation = write!(
    "delete-channel",
    "DeleteChannel",
    "Delete a notification channel",
    delete_channel,
    DELETE_INPUT,
    DestructiveWrite,
    &[CHANNEL_ID],
    &[],
    |input: &Input| sdk::DeleteChannelInput::new(&input.channel_id),
    std::convert::identity,
    &["Requires --yes. GreenNode removes the deleted channel from checks that used it."],
    "delete-channel --channel-id <channel-id> --yes"
);
pub(super) const SEND: Operation = write!(
    "send-channel-otp",
    "SendChannelOTP",
    "Send an OTP challenge",
    send_channel_otp,
    SEND_INPUT,
    Write,
    &[TYPE, ADDRESS],
    &[HEADERS],
    |input: &Input| {
        let mut value = sdk::SendChannelOtpInput::new(
            &input.r#type,
            input.address.as_deref().unwrap_or_default(),
        );
        value.headers = input.headers.clone().unwrap_or_default();
        value
    },
    std::convert::identity,
    &[
        "Use the returned Ref and the OTP received at the address to create or update a channel before it expires. Pass Slack addresses and all Headers through --cli-input-json file://channel.json. Webhook does not use an OTP."
    ],
    "send-channel-otp --type Email --address <address>"
);
