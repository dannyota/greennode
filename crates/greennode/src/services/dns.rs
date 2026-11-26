use clap::ArgMatches;
use greennode_sdk::dns::{
    Client, GetHostedZoneInput, GetRecordInput, ListHostedZonesInput, ListRecordsInput,
};
use greennode_sdk::{Config, RequestContext};

use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, Service};

const HOSTED_ZONE_ID: Field = Field {
    name: "hosted-zone-id",
    go_type: "string",
    required: true,
};
const RECORD_ID: Field = Field {
    name: "record-id",
    go_type: "string",
    required: true,
};
const NAME: Field = Field {
    name: "name",
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

fn check_input_source(_: &ArgMatches, json: Option<&str>) -> Result<(), InputError> {
    if json == Some("file://") {
        return Err(InputError("--cli-input-json: cannot open file"));
    }
    Ok(())
}

const GET_HOSTED_ZONE_INPUT: InputDescriptor<GetHostedZoneInput> = InputDescriptor {
    defaults: || GetHostedZoneInput::new(""),
    fields: &[InputField {
        json_name: "HostedZoneID",
        flag: Some(HOSTED_ZONE_ID),
        secret: false,
        nullable: false,
        set_json: |input, value| {
            let serde_json::Value::String(id) = value else {
                return Err(());
            };
            input.hosted_zone_id = id;
            Ok(())
        },
        set_flag: Some(|input, matches| {
            input.hosted_zone_id = matches
                .get_one::<String>(HOSTED_ZONE_ID.name)
                .ok_or(())?
                .clone();
            Ok(())
        }),
        requirement: Requirement::Nonzero(|input| !input.hosted_zone_id.is_empty()),
    }],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: None,
};

const GET_RECORD_INPUT: InputDescriptor<GetRecordInput> = InputDescriptor {
    defaults: || GetRecordInput::new("", ""),
    fields: &[
        InputField {
            json_name: "HostedZoneID",
            flag: Some(HOSTED_ZONE_ID),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                let serde_json::Value::String(id) = value else {
                    return Err(());
                };
                input.hosted_zone_id = id;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.hosted_zone_id = matches
                    .get_one::<String>(HOSTED_ZONE_ID.name)
                    .ok_or(())?
                    .clone();
                Ok(())
            }),
            requirement: Requirement::Nonzero(|input| !input.hosted_zone_id.is_empty()),
        },
        InputField {
            json_name: "RecordID",
            flag: Some(RECORD_ID),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                let serde_json::Value::String(id) = value else {
                    return Err(());
                };
                input.record_id = id;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.record_id = matches.get_one::<String>(RECORD_ID.name).ok_or(())?.clone();
                Ok(())
            }),
            requirement: Requirement::Nonzero(|input| !input.record_id.is_empty()),
        },
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: None,
};

const LIST_HOSTED_ZONES_INPUT: InputDescriptor<ListHostedZonesInput> = InputDescriptor {
    defaults: ListHostedZonesInput::default,
    fields: &[
        InputField {
            json_name: "Name",
            flag: Some(NAME),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                let serde_json::Value::String(name) = value else {
                    return Err(());
                };
                input.name = name;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.name = matches.get_one::<String>(NAME.name).ok_or(())?.clone();
                Ok(())
            }),
            requirement: Requirement::Optional,
        },
        InputField {
            json_name: "Page",
            flag: Some(PAGE),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.page = value.as_i64().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.page = *matches.get_one::<i64>(PAGE.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Optional,
        },
        InputField {
            json_name: "Size",
            flag: Some(SIZE),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.size = value.as_i64().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.size = *matches.get_one::<i64>(SIZE.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Optional,
        },
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: None,
};

const LIST_RECORDS_INPUT: InputDescriptor<ListRecordsInput> = InputDescriptor {
    defaults: || ListRecordsInput::new(""),
    fields: &[
        InputField {
            json_name: "HostedZoneID",
            flag: Some(HOSTED_ZONE_ID),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                let serde_json::Value::String(id) = value else {
                    return Err(());
                };
                input.hosted_zone_id = id;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.hosted_zone_id = matches
                    .get_one::<String>(HOSTED_ZONE_ID.name)
                    .ok_or(())?
                    .clone();
                Ok(())
            }),
            requirement: Requirement::Nonzero(|input| !input.hosted_zone_id.is_empty()),
        },
        InputField {
            json_name: "Name",
            flag: Some(NAME),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                let serde_json::Value::String(name) = value else {
                    return Err(());
                };
                input.name = name;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.name = matches.get_one::<String>(NAME.name).ok_or(())?.clone();
                Ok(())
            }),
            requirement: Requirement::Optional,
        },
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: None,
};

pub(super) const SERVICE: Service = Service {
    name: "dns",
    error_rules: &[],
    operations: &[
        Operation {
            name: "get-hosted-zone",
            sdk_operation: "dns.GetHostedZone",
            short_help: "Get a hosted zone",
            example: Some(
                "greennode dns get-hosted-zone --hosted-zone-id <hosted-zone-id> --query HostedZone",
            ),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[HOSTED_ZONE_ID],
            json_fields: &[],
            extra_flags: &[],
            notes: &[],
            handler: Some(Handler::new(
                &GET_HOSTED_ZONE_INPUT,
                |operation, matches, invocation| {
                    Box::pin(dispatch::management(
                        invocation,
                        operation,
                        matches,
                        &GET_HOSTED_ZONE_INPUT,
                        get_hosted_zone,
                    ))
                },
            )),
        },
        Operation {
            name: "get-record",
            sdk_operation: "dns.GetRecord",
            short_help: "Get a DNS record",
            example: Some(concat!(
                "greennode dns get-record --hosted-zone-id <hosted-zone-id> \\\n",
                "  --record-id <record-id> --query Record"
            )),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[HOSTED_ZONE_ID, RECORD_ID],
            json_fields: &[],
            extra_flags: &[],
            notes: &[],
            handler: Some(Handler::new(
                &GET_RECORD_INPUT,
                |operation, matches, invocation| {
                    Box::pin(dispatch::management(
                        invocation,
                        operation,
                        matches,
                        &GET_RECORD_INPUT,
                        get_record,
                    ))
                },
            )),
        },
        Operation {
            name: "list-hosted-zones",
            sdk_operation: "dns.ListHostedZones",
            short_help: "List hosted zones",
            example: None,
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[NAME, PAGE, SIZE],
            json_fields: &[],
            extra_flags: &[],
            notes: &["Reads one page. `--page` defaults to 1 and `--size` defaults to 10000."],
            handler: Some(Handler::new(
                &LIST_HOSTED_ZONES_INPUT,
                |operation, matches, invocation| {
                    Box::pin(dispatch::management(
                        invocation,
                        operation,
                        matches,
                        &LIST_HOSTED_ZONES_INPUT,
                        list_hosted_zones,
                    ))
                },
            )),
        },
        Operation {
            name: "list-records",
            sdk_operation: "dns.ListRecords",
            short_help: "List DNS records in a hosted zone",
            example: Some("greennode dns list-records --hosted-zone-id <hosted-zone-id>"),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[HOSTED_ZONE_ID, NAME],
            json_fields: &[],
            extra_flags: &[],
            notes: &[
                "Returns page metadata. If the service reports additional pages or rows, the\ncommand prints the partial page, ignores `--query`, and exits 1. Record\ncontinuation parameters remain unverified.",
            ],
            handler: Some(Handler::new(
                &LIST_RECORDS_INPUT,
                |operation, matches, invocation| {
                    Box::pin(dispatch::management(
                        invocation,
                        operation,
                        matches,
                        &LIST_RECORDS_INPUT,
                        list_records,
                    ))
                },
            )),
        },
    ],
};

fn get_hosted_zone<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a GetHostedZoneInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let result = Client::new(config)
            .get_hosted_zone(ctx, input.clone())
            .await;
        dispatch::project_result(result, std::convert::identity)
    })
}

fn get_record<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a GetRecordInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let result = Client::new(config).get_record(ctx, input.clone()).await;
        dispatch::project_result(result, std::convert::identity)
    })
}

fn list_hosted_zones<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a ListHostedZonesInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let result = Client::new(config)
            .list_hosted_zones(ctx, input.clone())
            .await;
        dispatch::project_result(result, std::convert::identity)
    })
}

fn list_records<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a ListRecordsInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let result = Client::new(config).list_records(ctx, input.clone()).await;
        dispatch::project_result(result, std::convert::identity)
    })
}
