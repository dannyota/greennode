use greennode_sdk::project::{Client, ListProjectsInput};
use greennode_sdk::{Config, RequestContext};

use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, Service};

const LIST_INPUT: InputDescriptor<ListProjectsInput> = InputDescriptor {
    defaults: ListProjectsInput::default,
    fields: &[InputField {
        json_name: "Region",
        flag: None,
        secret: false,
        nullable: false,
        set_json: |input, value| {
            let serde_json::Value::String(region) = value else {
                return Err(());
            };
            input.region = region;
            Ok(())
        },
        set_flag: None,
        requirement: Requirement::Optional,
    }],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(|_, json| {
        if json == Some("file://") {
            return Err(InputError("--cli-input-json: cannot open file"));
        }
        Ok(())
    }),
    guard: None,
};

pub(super) const SERVICE: Service = Service {
    name: "project",
    operations: &[Operation {
        name: "list-projects",
        sdk_operation: "project.ListProjects",
        short_help: "List projects",
        example: None,
        presence_required_fields: &[],
        inherited_bindings: &[],
        kind: Kind::Read,
        flags: &[],
        json_fields: &[Field {
            name: "Region",
            go_type: "string",
            required: false,
        }],
        extra_flags: &[],
        notes: &[],
        handler: Some(Handler::new(
            &LIST_INPUT,
            |operation, matches, invocation| {
                Box::pin(dispatch::management(
                    invocation,
                    operation,
                    matches,
                    &LIST_INPUT,
                    list_projects,
                ))
            },
        )),
    }],
};

fn list_projects<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a ListProjectsInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let result = Client::new(config).list_projects(ctx, input.clone()).await;
        dispatch::project_result(result, std::convert::identity)
    })
}
