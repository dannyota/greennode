use clap::ArgMatches;
use greennode_sdk::backup::{Client, GetPolicyInput, ListBackendsInput, ListPoliciesInput};
use greennode_sdk::{Config, RequestContext};

use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, Service};

const POLICY_ID: Field = Field {
    name: "policy-id",
    go_type: "string",
    required: true,
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

const GET_INPUT: InputDescriptor<GetPolicyInput> = InputDescriptor {
    defaults: || GetPolicyInput::new(""),
    fields: &[InputField {
        json_name: "PolicyID",
        flag: Some(POLICY_ID),
        secret: false,
        nullable: false,
        set_json: |input, value| {
            input.policy_id = match value {
                serde_json::Value::String(id) => id,
                _ => return Err(()),
            };
            Ok(())
        },
        set_flag: Some(|input, matches| {
            input.policy_id = matches.get_one::<String>(POLICY_ID.name).ok_or(())?.clone();
            Ok(())
        }),
        requirement: Requirement::Nonzero(|input| !input.policy_id.is_empty()),
    }],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: None,
};

const BACKENDS_INPUT: InputDescriptor<ListBackendsInput> = InputDescriptor {
    defaults: || ListBackendsInput,
    fields: &[],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: None,
};

const POLICIES_INPUT: InputDescriptor<ListPoliciesInput> = InputDescriptor {
    defaults: ListPoliciesInput::default,
    fields: &[
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

pub(super) const SERVICE: Service = Service {
    name: "backup",
    operations: &[
        Operation {
            name: "get-policy",
            sdk_operation: "backup.GetPolicy",
            short_help: "Get a Backup Center policy",
            example: Some("greennode backup get-policy --policy-id <policy-id> --query Policy"),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[POLICY_ID],
            json_fields: &[],
            extra_flags: &[],
            notes: &[
                "Reads one policy in `hcm-3` or `han-1`. `--project-id` has no effect. Hourly,\nweekly, and monthly schedule details are omitted.",
            ],
            handler: Some(Handler::new(
                &GET_INPUT,
                |operation, matches, invocation| {
                    Box::pin(dispatch::management(
                        invocation, operation, matches, &GET_INPUT, get_policy,
                    ))
                },
            )),
        },
        Operation {
            name: "list-backends",
            sdk_operation: "backup.ListBackends",
            short_help: "List Backup Center backends",
            example: None,
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[],
            json_fields: &[],
            extra_flags: &[],
            notes: &["Reads token-scoped backends in `hcm-3` or `han-1`."],
            handler: Some(Handler::new(
                &BACKENDS_INPUT,
                |operation, matches, invocation| {
                    Box::pin(dispatch::management(
                        invocation,
                        operation,
                        matches,
                        &BACKENDS_INPUT,
                        list_backends,
                    ))
                },
            )),
        },
        Operation {
            name: "list-policies",
            sdk_operation: "backup.ListPolicies",
            short_help: "List Backup Center policies",
            example: Some(concat!(
                "greennode backup list-policies \\\n",
                "  --query 'Items[].{ID:ID,Name:Name,Default:IsDefault}' --output table"
            )),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[PAGE, SIZE],
            json_fields: &[],
            extra_flags: &[],
            notes: &[
                "Reads one token-scoped page in `hcm-3` or `han-1`. `--page` defaults to 1 and\n`--size` defaults to 200.",
                "The output includes cadence enable flags and daily schedule settings.",
            ],
            handler: Some(Handler::new(
                &POLICIES_INPUT,
                |operation, matches, invocation| {
                    Box::pin(dispatch::management(
                        invocation,
                        operation,
                        matches,
                        &POLICIES_INPUT,
                        list_policies,
                    ))
                },
            )),
        },
    ],
};

fn get_policy<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a GetPolicyInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let result = Client::new(config).get_policy(ctx, input.clone()).await;
        dispatch::project_result(result, std::convert::identity)
    })
}

fn list_backends<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a ListBackendsInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let result = Client::new(config).list_backends(ctx, *input).await;
        dispatch::project_result(result, std::convert::identity)
    })
}

fn list_policies<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a ListPoliciesInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let result = Client::new(config).list_policies(ctx, *input).await;
        dispatch::project_result(result, std::convert::identity)
    })
}
