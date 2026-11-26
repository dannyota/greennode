use greennode_sdk::vks::{Client, DeleteClusterInput};
use greennode_sdk::{Config, RequestContext};

use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};

const CLUSTER_ID: Field = Field {
    name: "cluster-id",
    go_type: "string",
    required: true,
};
const NO_WAIT: Field = Field {
    name: "no-wait",
    go_type: "bool",
    required: false,
};

#[derive(Default)]
struct Input {
    cluster_id: String,
    no_wait: bool,
}

const INPUT: InputDescriptor<Input> = InputDescriptor {
    defaults: Input::default,
    fields: &[
        InputField {
            json_name: "ClusterID",
            flag: Some(CLUSTER_ID),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.cluster_id = value.as_str().ok_or(())?.to_owned();
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.cluster_id = matches
                    .get_one::<String>(CLUSTER_ID.name)
                    .ok_or(())?
                    .clone();
                Ok(())
            }),
            requirement: Requirement::Nonzero(|input| !input.cluster_id.is_empty()),
        },
        InputField {
            json_name: "NoWait",
            flag: Some(NO_WAIT),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.no_wait = value.as_bool().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.no_wait = *matches.get_one::<bool>(NO_WAIT.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Optional,
        },
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(super::check_input_source),
    guard: None,
};

pub(super) const DELETE_CLUSTER: Operation = Operation {
    name: "delete-cluster",
    sdk_operation: "vks.DeleteCluster",
    short_help: "Delete a Kubernetes cluster",
    example: None,
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::DestructiveWrite,
    flags: &[CLUSTER_ID, NO_WAIT],
    json_fields: &[],
    extra_flags: &[],
    notes: &[
        "Deletes the cluster named by exact ID, public or private. A 404 after a successful preflight read confirms deletion. Deletion does not promise a refund.",
    ],
    handler: Some(Handler::new(&INPUT, |operation, matches, invocation| {
        Box::pin(dispatch::management(
            invocation,
            operation,
            matches,
            &INPUT,
            delete_cluster,
        ))
    })),
};

fn delete_cluster<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a Input,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let input = DeleteClusterInput {
            cluster_id: input.cluster_id.clone(),
            no_wait: input.no_wait,
        };
        dispatch::project_result(
            Client::new(config).delete_cluster(ctx, input).await,
            std::convert::identity,
        )
    })
}
