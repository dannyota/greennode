use clap::ArgMatches;
use greennode_sdk::vks::{Client, GetQuotaInput, ListClusterVersionsInput, ListClustersInput};
use greennode_sdk::{Config, RequestContext};

use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, Service};

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

const LIST_INPUT: InputDescriptor<ListClustersInput> = InputDescriptor {
    defaults: ListClustersInput::default,
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

const VERSIONS_INPUT: InputDescriptor<ListClusterVersionsInput> = InputDescriptor {
    defaults: ListClusterVersionsInput::default,
    fields: &[],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: None,
};

const QUOTA_INPUT: InputDescriptor<GetQuotaInput> = InputDescriptor {
    defaults: GetQuotaInput::default,
    fields: &[],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: None,
};

const SCOPE: &str = "VKS reads work in `hcm-3` and `han-1` and support `--read-only`. `--project-id`\n\
    does not select a VKS workspace. Output contains account data.";

pub(super) const SERVICE: Service = Service {
    name: "vks",
    error_rules: &[],
    operations: &[
        Operation {
            name: "list-clusters",
            sdk_operation: "vks.ListClusters",
            short_help: "List Kubernetes clusters",
            example: Some(concat!(
                "greennode vks list-clusters \\\n",
                "  --query 'Items[].{ID:ID,Name:Name,Status:Status,Version:Version}' \\\n",
                "  --output table",
            )),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[PAGE, SIZE],
            json_fields: &[],
            extra_flags: &[],
            notes: &[
                "Pages start at 0, with a default size of 10. Each call reads one page. Request\n\
                 the next page with `--page 1`. Use `--size` to set the page size.",
                SCOPE,
                "Every list prints `{\"Items\": [...]}`, so a `--query` starts with `Items`.",
            ],
            handler: Some(Handler::new(
                &LIST_INPUT,
                |operation, matches, invocation| {
                    Box::pin(dispatch::management(
                        invocation,
                        operation,
                        matches,
                        &LIST_INPUT,
                        list_clusters,
                    ))
                },
            )),
        },
        Operation {
            name: "list-cluster-versions",
            sdk_operation: "vks.ListClusterVersions",
            short_help: "List Kubernetes cluster versions",
            example: None,
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[],
            json_fields: &[],
            extra_flags: &[],
            notes: &[],
            handler: Some(Handler::new(
                &VERSIONS_INPUT,
                |operation, matches, invocation| {
                    Box::pin(dispatch::management(
                        invocation,
                        operation,
                        matches,
                        &VERSIONS_INPUT,
                        list_cluster_versions,
                    ))
                },
            )),
        },
        Operation {
            name: "get-quota",
            sdk_operation: "vks.GetQuota",
            short_help: "Get Kubernetes account quota",
            example: Some("greennode vks get-quota --query Quota"),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[],
            json_fields: &[],
            extra_flags: &[],
            notes: &[],
            handler: Some(Handler::new(
                &QUOTA_INPUT,
                |operation, matches, invocation| {
                    Box::pin(dispatch::management(
                        invocation,
                        operation,
                        matches,
                        &QUOTA_INPUT,
                        get_quota,
                    ))
                },
            )),
        },
    ],
};

fn check_input_source(_: &ArgMatches, json: Option<&str>) -> Result<(), InputError> {
    if json == Some("file://") {
        return Err(InputError("--cli-input-json: cannot open file"));
    }
    Ok(())
}

fn list_clusters<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a ListClustersInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let result = Client::new(config).list_clusters(ctx, *input).await;
        dispatch::project_result(result, std::convert::identity)
    })
}

fn list_cluster_versions<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a ListClusterVersionsInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let result = Client::new(config).list_cluster_versions(ctx, *input).await;
        dispatch::project_result(result, std::convert::identity)
    })
}

fn get_quota<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a GetQuotaInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let result = Client::new(config).get_quota(ctx, *input).await;
        dispatch::project_result(result, std::convert::identity)
    })
}
