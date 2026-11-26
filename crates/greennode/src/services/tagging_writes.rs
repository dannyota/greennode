use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};
use greennode_sdk::tagging::{Client, TagResourceInput, UntagResourceInput};
use greennode_sdk::{Config, RequestContext};

const RESOURCE_ID: Field = Field {
    name: "resource-id",
    go_type: "string",
    required: true,
};
const RESOURCE_TYPE: Field = Field {
    name: "resource-type",
    ..RESOURCE_ID
};
const KEY: Field = Field {
    name: "key",
    ..RESOURCE_ID
};
const VALUE: Field = Field {
    name: "value",
    required: false,
    ..RESOURCE_ID
};

#[derive(Default)]
struct Input {
    resource_id: String,
    resource_type: String,
    key: String,
    value: String,
}

macro_rules! field {
    ($json:literal, $flag:ident, $field:ident) => {
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
            requirement: if $flag.required {
                Requirement::Nonzero(|input| !input.$field.is_empty())
            } else {
                Requirement::Optional
            },
        }
    };
}

const TAG_INPUT: InputDescriptor<Input> = InputDescriptor {
    defaults: Input::default,
    fields: &[
        field!("ResourceID", RESOURCE_ID, resource_id),
        field!("ResourceType", RESOURCE_TYPE, resource_type),
        field!("Key", KEY, key),
        field!("Value", VALUE, value),
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: None,
    guard: None,
};
const UNTAG_INPUT: InputDescriptor<Input> = InputDescriptor {
    fields: &[
        field!("ResourceID", RESOURCE_ID, resource_id),
        field!("ResourceType", RESOURCE_TYPE, resource_type),
        field!("Key", KEY, key),
    ],
    ..TAG_INPUT
};

pub(super) const TAG: Operation = Operation {
    name: "tag-resource",
    sdk_operation: "tagging.TagResource",
    short_help: "Set a resource tag",
    example: None,
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Write,
    flags: &[RESOURCE_ID, RESOURCE_TYPE, KEY, VALUE],
    json_fields: &[],
    extra_flags: &[],
    notes: &[],
    handler: Some(Handler::new(
        &TAG_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management(
                invocation, operation, matches, &TAG_INPUT, tag,
            ))
        },
    )),
};
pub(super) const UNTAG: Operation = Operation {
    name: "untag-resource",
    sdk_operation: "tagging.UntagResource",
    short_help: "Remove a resource tag",
    example: None,
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Write,
    flags: &[RESOURCE_ID, RESOURCE_TYPE, KEY],
    json_fields: &[],
    extra_flags: &[],
    notes: &[],
    handler: Some(Handler::new(
        &UNTAG_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management(
                invocation,
                operation,
                matches,
                &UNTAG_INPUT,
                untag,
            ))
        },
    )),
};

#[derive(serde::Serialize)]
#[serde(rename_all = "PascalCase")]
struct ProjectedTag {
    key: serde_json::Value,
    value: serde_json::Value,
    system_tag: serde_json::Value,
    created_at: serde_json::Value,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "PascalCase")]
struct ProjectedOutput {
    tags: Vec<ProjectedTag>,
    previous: serde_json::Value,
    changed: serde_json::Value,
}

fn project(mut value: serde_json::Value) -> ProjectedOutput {
    let tags = value["Tags"]
        .as_array_mut()
        .map(|tags| {
            tags.iter_mut()
                .map(|tag| ProjectedTag {
                    key: tag["Key"].take(),
                    value: tag["Value"].take(),
                    system_tag: tag["SystemTag"].take(),
                    created_at: tag["CreatedAt"].take(),
                })
                .collect()
        })
        .unwrap_or_default();
    ProjectedOutput {
        tags,
        previous: value["Previous"].take(),
        changed: value["Changed"].take(),
    }
}

fn tag<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a Input,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            Client::new(config)
                .tag_resource(
                    ctx,
                    TagResourceInput {
                        resource_id: input.resource_id.clone(),
                        resource_type: input.resource_type.clone(),
                        key: input.key.clone(),
                        value: input.value.clone(),
                    },
                )
                .await,
            project,
        )
    })
}
fn untag<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a Input,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            Client::new(config)
                .untag_resource(
                    ctx,
                    UntagResourceInput {
                        resource_id: input.resource_id.clone(),
                        resource_type: input.resource_type.clone(),
                        key: input.key.clone(),
                    },
                )
                .await,
            project,
        )
    })
}
