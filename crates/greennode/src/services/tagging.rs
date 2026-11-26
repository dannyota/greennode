#[path = "tagging_writes.rs"]
mod writes;

use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::errors::{Rule, RuleExit, RuleName, RuleOperation};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, Service};
use greennode_sdk::tagging::{Client, ListResourceTagsInput};
use greennode_sdk::{Config, RequestContext};

const RESOURCE_ID: Field = Field {
    name: "resource-id",
    go_type: "string",
    required: true,
};

const INPUT: InputDescriptor<ListResourceTagsInput> = InputDescriptor {
    defaults: || ListResourceTagsInput::new(""),
    fields: &[InputField {
        json_name: "ResourceID",
        flag: Some(RESOURCE_ID),
        secret: false,
        nullable: false,
        set_json: |input, value| {
            let serde_json::Value::String(value) = value else {
                return Err(());
            };
            input.resource_id = value;
            Ok(())
        },
        set_flag: Some(|input, matches| {
            input.resource_id = matches
                .get_one::<String>(RESOURCE_ID.name)
                .ok_or(())?
                .clone();
            Ok(())
        }),
        requirement: Requirement::Nonzero(|input| !input.resource_id.is_empty()),
    }],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(|_, json| {
        if json == Some("file://") {
            Err(InputError("--cli-input-json: cannot open file"))
        } else {
            Ok(())
        }
    }),
    guard: None,
};

pub(super) const SERVICE: Service = Service {
    name: "tagging",
    error_rules: &[
        Rule {
            identity: "tagging-system-tag",
            matches: |error| {
                error.detail::<greennode_sdk::tagging::ErrorDetail>()
                    == Some(&greennode_sdk::tagging::ErrorDetail::SystemTag)
            },
            name: Some(RuleName {
                rank: 10,
                code: "SystemTag",
                operation: RuleOperation::Omit,
                include_api_status: false,
            }),
            exit: Some(RuleExit::Failure),
            message: Some("tagging: key is a system tag"),
        },
        Rule {
            identity: "tagging-not-settled",
            matches: |error| {
                error.detail::<greennode_sdk::tagging::ErrorDetail>()
                    == Some(&greennode_sdk::tagging::ErrorDetail::NotSettled)
            },
            name: None,
            exit: Some(RuleExit::Failure),
            message: Some("tagging: write outcome is unconfirmed; read tags before retrying"),
        },
    ],
    operations: &[
        writes::TAG,
        writes::UNTAG,
        Operation {
            name: "list-resource-tags",
            sdk_operation: "tagging.ListResourceTags",
            short_help: "List resource tags",
            example: Some("greennode tagging list-resource-tags --resource-id <resource-id>"),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[RESOURCE_ID],
            json_fields: &[],
            extra_flags: &[],
            notes: &[
                "Returns every tag on the resource, system tags included: vng.zone, vng.region, and vng.createdBy are confirmed live on a virtual IP address. Tag.SystemTag marks one of them.",
            ],
            handler: Some(Handler::new(&INPUT, |operation, matches, invocation| {
                Box::pin(dispatch::management(
                    invocation, operation, matches, &INPUT, list,
                ))
            })),
        },
    ],
};

fn list<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a ListResourceTagsInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            Client::new(config)
                .list_resource_tags(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}
