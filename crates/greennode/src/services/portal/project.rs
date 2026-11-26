use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};
use crate::cli::render::DynamicMap;
use greennode_sdk::portal::{
    Client, GetQuotaInput, GetTagQuotaInput, ListQuotaUsedInput, ListZonesInput,
};
use greennode_sdk::{Config, RequestContext};
use serde::Serialize;

fn source(_: &clap::ArgMatches, json: Option<&str>) -> Result<(), InputError> {
    if json == Some("file://") {
        return Err(InputError("--cli-input-json: cannot open file"));
    }
    Ok(())
}

macro_rules! empty_input {
    ($name:ident, $ty:ty) => {
        const $name: InputDescriptor<$ty> = InputDescriptor {
            defaults: <$ty>::default,
            fields: &[],
            allow_json: true,
            allow_query: true,
            project_binding: None,
            pre_guard: Some(source),
            guard: None,
        };
    };
}
empty_input!(ZONES_INPUT, ListZonesInput);
empty_input!(QUOTAS_INPUT, ListQuotaUsedInput);
empty_input!(TAG_INPUT, GetTagQuotaInput);
const NAME: Field = Field {
    name: "name",
    go_type: "string",
    required: true,
};
const QUOTA_INPUT: InputDescriptor<GetQuotaInput> = InputDescriptor {
    defaults: GetQuotaInput::default,
    fields: &[InputField {
        json_name: "Name",
        flag: Some(NAME),
        secret: false,
        nullable: false,
        set_json: |input, value| {
            let serde_json::Value::String(value) = value else {
                return Err(());
            };
            input.name = value;
            Ok(())
        },
        set_flag: Some(|input, matches| {
            input.name = matches.get_one::<String>("name").ok_or(())?.clone();
            Ok(())
        }),
        requirement: Requirement::Nonzero(|input| !input.name.is_empty()),
    }],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(source),
    guard: None,
};

macro_rules! op {
    ($constant:ident, $name:literal, $sdk:literal, $help:literal, $input:ident, $handler:ident, $flags:expr) => {
        pub(super) const $constant: Operation = Operation {
            name: $name,
            sdk_operation: $sdk,
            short_help: $help,
            example: None,
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: $flags,
            json_fields: &[],
            extra_flags: &[],
            notes: &[],
            handler: Some(Handler::new(&$input, |operation, matches, invocation| {
                Box::pin(dispatch::management(
                    invocation, operation, matches, &$input, $handler,
                ))
            })),
        };
    };
}
op!(
    ZONES,
    "list-zones",
    "portal.ListZones",
    "List project zones",
    ZONES_INPUT,
    zones,
    &[]
);
op!(
    QUOTAS,
    "list-quota-used",
    "portal.ListQuotaUsed",
    "List project quotas",
    QUOTAS_INPUT,
    quotas,
    &[]
);
op!(
    QUOTA,
    "get-quota",
    "portal.GetQuota",
    "Get a project quota",
    QUOTA_INPUT,
    quota,
    &[NAME]
);
op!(
    TAG,
    "get-tag-quota",
    "portal.GetTagQuota",
    "Get the project tag quota",
    TAG_INPUT,
    tag,
    &[]
);

#[derive(Serialize)]
struct Maps {
    #[serde(rename = "Items")]
    items: Vec<DynamicMap<greennode_sdk::portal::Quota>>,
}
#[derive(Serialize)]
struct Quota {
    #[serde(rename = "Quota")]
    quota: DynamicMap<greennode_sdk::portal::Quota>,
}
#[derive(Serialize)]
struct Tag {
    #[serde(rename = "TagQuota")]
    tag: DynamicMap<greennode_sdk::portal::TagQuota>,
}
fn zones<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a ListZonesInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            Client::new(config).list_zones(ctx, input.clone()).await,
            std::convert::identity,
        )
    })
}
fn quotas<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a ListQuotaUsedInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            Client::new(config)
                .list_quota_used(ctx, input.clone())
                .await,
            |o| Maps {
                items: o.items.into_iter().map(DynamicMap).collect(),
            },
        )
    })
}
fn quota<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a GetQuotaInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            Client::new(config).get_quota(ctx, input.clone()).await,
            |o| Quota {
                quota: DynamicMap(o.quota),
            },
        )
    })
}
fn tag<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a GetTagQuotaInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            Client::new(config).get_tag_quota(ctx, input.clone()).await,
            |o| Tag {
                tag: DynamicMap(o.tag_quota),
            },
        )
    })
}
