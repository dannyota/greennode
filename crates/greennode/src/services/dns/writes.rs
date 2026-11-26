use greennode_sdk::dns::{
    Client, CreateHostedZoneInput, CreateRecordInput, DeleteHostedZoneInput, DeleteRecordInput,
    Field as DnsField, RecordValue, UpdateHostedZoneInput, UpdateRecordInput,
};
use greennode_sdk::{Config, RequestContext};
use serde::Deserialize;

use super::{HOSTED_ZONE_ID, RECORD_ID, check_input_source};
use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};

const fn field(name: &'static str, go_type: &'static str, required: bool) -> Field {
    Field {
        name,
        go_type,
        required,
    }
}

const DOMAIN: Field = field("domain-name", "string", true);
const VPCS: Field = field("vpc-ids", "[]string", true);
const UPDATE_VPCS: Field = field("vpc-ids", "*[]string", false);
const DESCRIPTION: Field = field("description", "string", false);
const UPDATE_DESCRIPTION: Field = field("description", "*string", false);
const NO_WAIT: Field = field("no-wait", "bool", false);
const TYPE: Field = field("type", "string", true);
const UPDATE_TYPE: Field = field("type", "*string", false);
const SUB_DOMAIN: Field = field("sub-domain", "string", false);
const UPDATE_SUB_DOMAIN: Field = field("sub-domain", "*string", false);
const TTL: Field = field("ttl", "int", false);
const UPDATE_TTL: Field = field("ttl", "*int", false);
const ROUTING: Field = field("routing-policy", "string", false);
const UPDATE_ROUTING: Field = field("routing-policy", "*string", false);
const STICKY: Field = field("sticky-session", "*bool", false);
const VALUES: Field = field("Values", "[]dns.RecordValue", true);
const UPDATE_VALUES: Field = field("Values", "*[]dns.RecordValue", false);

macro_rules! input_field {
    ($name:literal, $flag:expr, $member:ident, $shape:ident, $requirement:expr) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$member = input_field!(@wrap $shape, serde_json::from_value(value).map_err(|_| ())?);
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$member = input_field!(@wrap $shape, input_field!(@flag $shape, matches, $flag));
                Ok(())
            }),
            requirement: $requirement,
        }
    };
    (@flag scalar, $matches:ident, $flag:expr) => { $matches.get_one($flag.name).cloned().ok_or(())? };
    (@flag pointer, $matches:ident, $flag:expr) => { input_field!(@flag scalar, $matches, $flag) };
    (@flag array, $matches:ident, $flag:expr) => { $matches.get_many::<String>($flag.name).ok_or(())?.cloned().collect() };
    (@flag array_pointer, $matches:ident, $flag:expr) => { input_field!(@flag array, $matches, $flag) };
    (@wrap scalar, $value:expr) => { $value };
    (@wrap pointer, $value:expr) => { Some($value) };
    (@wrap array, $value:expr) => { $value };
    (@wrap array_pointer, $value:expr) => { Some($value) };
}

macro_rules! zone_id {
    () => {
        input_field!(
            "HostedZoneID",
            HOSTED_ZONE_ID,
            hosted_zone_id,
            scalar,
            Requirement::Nonzero(|input| !input.hosted_zone_id.is_empty())
        )
    };
}

macro_rules! record_id {
    () => {
        input_field!(
            "RecordID",
            RECORD_ID,
            record_id,
            scalar,
            Requirement::Nonzero(|input| !input.record_id.is_empty())
        )
    };
}

macro_rules! no_wait {
    () => {
        input_field!("NoWait", NO_WAIT, no_wait, scalar, Requirement::Optional)
    };
}

const fn descriptor<T>(
    defaults: fn() -> T,
    fields: &'static [InputField<T>],
) -> InputDescriptor<T> {
    InputDescriptor {
        defaults,
        fields,
        allow_json: true,
        allow_query: true,
        project_binding: None,
        pre_guard: Some(check_input_source),
        guard: None,
    }
}

const CREATE_ZONE_INPUT: InputDescriptor<CreateHostedZoneInput> = descriptor(
    || CreateHostedZoneInput::new("", vec![]),
    &[
        input_field!(
            "DomainName",
            DOMAIN,
            domain_name,
            scalar,
            Requirement::Nonzero(|input| !input.domain_name.is_empty())
        ),
        input_field!(
            "VPCIDs",
            VPCS,
            vpc_ids,
            array,
            Requirement::Nonzero(|input| !input.vpc_ids.is_empty())
        ),
        input_field!(
            "Description",
            DESCRIPTION,
            description,
            scalar,
            Requirement::Optional
        ),
        no_wait!(),
    ],
);

const UPDATE_ZONE_INPUT: InputDescriptor<UpdateHostedZoneInput> = descriptor(
    || UpdateHostedZoneInput::new(""),
    &[
        zone_id!(),
        input_field!(
            "VPCIDs",
            UPDATE_VPCS,
            vpc_ids,
            array_pointer,
            Requirement::Optional
        ),
        input_field!(
            "Description",
            UPDATE_DESCRIPTION,
            description,
            pointer,
            Requirement::Optional
        ),
        no_wait!(),
    ],
);

const DELETE_ZONE_INPUT: InputDescriptor<DeleteHostedZoneInput> =
    descriptor(|| DeleteHostedZoneInput::new(""), &[zone_id!(), no_wait!()]);

const CREATE_RECORD_INPUT: InputDescriptor<CreateRecordInput> = descriptor(
    || CreateRecordInput::new("", "", vec![]),
    &[
        zone_id!(),
        input_field!(
            "Type",
            TYPE,
            record_type,
            scalar,
            Requirement::Nonzero(|input| !input.record_type.is_empty())
        ),
        InputField {
            json_name: "Values",
            flag: None,
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.values = values(value)?;
                Ok(())
            },
            set_flag: None,
            requirement: Requirement::Nonzero(|input| !input.values.is_empty()),
        },
        input_field!(
            "SubDomain",
            SUB_DOMAIN,
            sub_domain,
            scalar,
            Requirement::Optional
        ),
        input_field!("TTL", TTL, ttl, scalar, Requirement::Optional),
        input_field!(
            "RoutingPolicy",
            ROUTING,
            routing_policy,
            scalar,
            Requirement::Optional
        ),
        input_field!(
            "StickySession",
            STICKY,
            sticky_session,
            pointer,
            Requirement::Optional
        ),
        no_wait!(),
    ],
);

const UPDATE_RECORD_INPUT: InputDescriptor<UpdateRecordInput> = descriptor(
    || UpdateRecordInput::new("", ""),
    &[
        zone_id!(),
        record_id!(),
        input_field!(
            "SubDomain",
            UPDATE_SUB_DOMAIN,
            sub_domain,
            pointer,
            Requirement::Optional
        ),
        input_field!(
            "Type",
            UPDATE_TYPE,
            record_type,
            pointer,
            Requirement::Optional
        ),
        input_field!("TTL", UPDATE_TTL, ttl, pointer, Requirement::Optional),
        input_field!(
            "RoutingPolicy",
            UPDATE_ROUTING,
            routing_policy,
            pointer,
            Requirement::Optional
        ),
        InputField {
            json_name: "Values",
            flag: None,
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.values = Some(values(value)?);
                Ok(())
            },
            set_flag: None,
            requirement: Requirement::Optional,
        },
        input_field!(
            "StickySession",
            STICKY,
            sticky_session,
            pointer,
            Requirement::Optional
        ),
        no_wait!(),
    ],
);

const DELETE_RECORD_INPUT: InputDescriptor<DeleteRecordInput> = descriptor(
    || DeleteRecordInput::new("", ""),
    &[zone_id!(), record_id!(), no_wait!()],
);

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct JsonRecordValue {
    #[serde(alias = "Value")]
    value: Option<String>,
    #[serde(alias = "Location")]
    location: Option<String>,
    #[serde(alias = "Weight")]
    weight: Option<i64>,
}

fn values(value: serde_json::Value) -> Result<Vec<RecordValue>, ()> {
    let values: Vec<Option<JsonRecordValue>> = serde_json::from_value(value).map_err(|_| ())?;
    Ok(values
        .into_iter()
        .map(|value| {
            let value = value.unwrap_or_default();
            RecordValue {
                value: value.value.unwrap_or_default(),
                location: value.location,
                weight: value.weight.map(DnsField::Value),
            }
        })
        .collect())
}

const WAIT_NOTE: &str = "Each readiness or settlement wait shares the earlier of the caller's deadline and 60 seconds. Sends the write once. Read the current state before another write if the outcome is uncertain.";
const REPLACEMENT_NOTE: &str = "Reads a matching zone with a description and a non-null VPC list before replacing either field. Use `--cli-input-json '{\"VPCIDs\":[]}'` to detach all VPCs.";

macro_rules! write {
    ($name:ident, $input:ident, $descriptor:ident, $method:ident, $command:literal, $sdk:literal, $help:literal, $kind:ident,
        $flags:expr, $json:expr, $example:expr, $notes:expr) => {
        pub(super) const $name: Operation = Operation {
            name: $command,
            sdk_operation: $sdk,
            short_help: $help,
            example: Some($example),
            kind: Kind::$kind,
            flags: $flags,
            json_fields: $json,
            extra_flags: &[],
            notes: $notes,
            presence_required_fields: &[],
            inherited_bindings: &[],
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
        };

        fn $method<'a>(
            ctx: &'a RequestContext,
            config: &'a Config,
            input: &'a $input,
        ) -> FutureResult<'a, ExecutionResult> {
            Box::pin(async move {
                let result = Client::new(config).$method(ctx, input.clone()).await;
                dispatch::project_result(result, std::convert::identity)
            })
        }
    };
}

write!(
    CREATE_ZONE,
    CreateHostedZoneInput,
    CREATE_ZONE_INPUT,
    create_hosted_zone,
    "create-hosted-zone",
    "dns.CreateHostedZone",
    "Create a private hosted zone",
    Write,
    &[DOMAIN, VPCS, DESCRIPTION, NO_WAIT],
    &[],
    "greennode dns create-hosted-zone --domain-name <domain-name> --vpc-ids <vpc-id>",
    &[WAIT_NOTE]
);
write!(
    UPDATE_ZONE,
    UpdateHostedZoneInput,
    UPDATE_ZONE_INPUT,
    update_hosted_zone,
    "update-hosted-zone",
    "dns.UpdateHostedZone",
    "Replace hosted zone fields",
    Write,
    &[HOSTED_ZONE_ID, UPDATE_VPCS, UPDATE_DESCRIPTION, NO_WAIT],
    &[],
    "greennode dns update-hosted-zone --hosted-zone-id <hosted-zone-id> --description <description>",
    &[REPLACEMENT_NOTE, WAIT_NOTE]
);
write!(
    DELETE_ZONE,
    DeleteHostedZoneInput,
    DELETE_ZONE_INPUT,
    delete_hosted_zone,
    "delete-hosted-zone",
    "dns.DeleteHostedZone",
    "Delete a hosted zone",
    DestructiveWrite,
    &[HOSTED_ZONE_ID, NO_WAIT],
    &[],
    "greennode dns delete-hosted-zone --hosted-zone-id <hosted-zone-id> --yes",
    &[WAIT_NOTE]
);
write!(
    CREATE_RECORD,
    CreateRecordInput,
    CREATE_RECORD_INPUT,
    create_record,
    "create-record",
    "dns.CreateRecord",
    "Create a DNS record",
    Write,
    &[
        HOSTED_ZONE_ID,
        TYPE,
        SUB_DOMAIN,
        TTL,
        ROUTING,
        STICKY,
        NO_WAIT
    ],
    &[VALUES],
    "greennode dns create-record --hosted-zone-id <hosted-zone-id> --type <type> --cli-input-json '{\"Values\":[{\"Value\":\"<value>\"}]}'",
    &[WAIT_NOTE]
);
write!(
    UPDATE_RECORD,
    UpdateRecordInput,
    UPDATE_RECORD_INPUT,
    update_record,
    "update-record",
    "dns.UpdateRecord",
    "Update a DNS record",
    Write,
    &[
        HOSTED_ZONE_ID,
        RECORD_ID,
        UPDATE_SUB_DOMAIN,
        UPDATE_TYPE,
        UPDATE_TTL,
        UPDATE_ROUTING,
        STICKY,
        NO_WAIT
    ],
    &[UPDATE_VALUES],
    "greennode dns update-record --hosted-zone-id <hosted-zone-id> --record-id <record-id> --ttl <ttl>",
    &[
        WAIT_NOTE,
        "TXT values compare exactly during settlement. `--no-wait` still reads the record once after the update."
    ]
);
write!(
    DELETE_RECORD,
    DeleteRecordInput,
    DELETE_RECORD_INPUT,
    delete_record,
    "delete-record",
    "dns.DeleteRecord",
    "Delete a DNS record",
    DestructiveWrite,
    &[HOSTED_ZONE_ID, RECORD_ID, NO_WAIT],
    &[],
    "greennode dns delete-record --hosted-zone-id <hosted-zone-id> --record-id <record-id> --yes",
    &[WAIT_NOTE]
);
