use greennode_sdk::billing::{
    Client, GetBalancesInput, GetCostOverviewInput, ListCostResourcesInput,
};
use greennode_sdk::{Config, RequestContext};

use super::{check_input_source, json_string};
use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};

#[derive(Default)]
struct CostInput {
    start_date: String,
    end_date: String,
    product: String,
    resource_type: String,
    resource_id: String,
    query: String,
    group_by: String,
    interval: String,
    page: i64,
    size: i64,
    sort: String,
    order: String,
}

macro_rules! string_field {
    ($name:literal, $json:literal, $field:ident, $required:literal, $requirement:expr) => {
        InputField {
            json_name: $json,
            flag: Some(Field {
                name: $name,
                go_type: "string",
                required: $required,
            }),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$field = json_string(value)?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$field = matches.get_one::<String>($name).ok_or(())?.clone();
                Ok(())
            }),
            requirement: $requirement,
        }
    };
    ($name:literal, $json:literal, $field:ident) => {
        string_field!($name, $json, $field, false, Requirement::Optional)
    };
}

macro_rules! int_field {
    ($name:literal, $json:literal, $field:ident) => {
        InputField {
            json_name: $json,
            flag: Some(Field {
                name: $name,
                go_type: "int",
                required: false,
            }),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$field = value.as_i64().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$field = *matches.get_one::<i64>($name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Optional,
        }
    };
}

const START_DATE: InputField<CostInput> = string_field!(
    "start-date",
    "StartDate",
    start_date,
    true,
    Requirement::Nonzero(|input| !input.start_date.is_empty())
);
const END_DATE: InputField<CostInput> = string_field!(
    "end-date",
    "EndDate",
    end_date,
    true,
    Requirement::Nonzero(|input| !input.end_date.is_empty())
);
const PRODUCT: InputField<CostInput> = string_field!("product", "Product", product);
const RESOURCE_TYPE: InputField<CostInput> =
    string_field!("resource-type", "ResourceType", resource_type);
const RESOURCE_ID: InputField<CostInput> = string_field!("resource-id", "ResourceID", resource_id);
const SEARCH: InputField<CostInput> = string_field!("search", "Query", query);
const GROUP_BY: InputField<CostInput> = string_field!("group-by", "GroupBy", group_by);
const INTERVAL: InputField<CostInput> = string_field!("interval", "Interval", interval);
const PAGE: InputField<CostInput> = int_field!("page", "Page", page);
const SIZE: InputField<CostInput> = int_field!("size", "Size", size);
const SORT: InputField<CostInput> = string_field!("sort", "Sort", sort);
const ORDER: InputField<CostInput> = string_field!("order", "Order", order);

const OVERVIEW_FIELDS: [InputField<CostInput>; 8] = [
    START_DATE,
    END_DATE,
    PRODUCT,
    RESOURCE_TYPE,
    RESOURCE_ID,
    SEARCH,
    GROUP_BY,
    INTERVAL,
];
const RESOURCE_FIELDS: [InputField<CostInput>; 10] = [
    START_DATE,
    END_DATE,
    PRODUCT,
    RESOURCE_TYPE,
    RESOURCE_ID,
    SEARCH,
    PAGE,
    SIZE,
    SORT,
    ORDER,
];

const fn flags<const N: usize>(fields: &[InputField<CostInput>; N]) -> [Field; N] {
    let mut flags = [Field {
        name: "",
        go_type: "",
        required: false,
    }; N];
    let mut index = 0;
    while index < N {
        if let Some(flag) = fields[index].flag {
            flags[index] = flag;
        }
        index += 1;
    }
    flags
}

const OVERVIEW_INPUT: InputDescriptor<CostInput> = InputDescriptor {
    defaults: CostInput::default,
    fields: &OVERVIEW_FIELDS,
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: None,
};

const RESOURCES_INPUT: InputDescriptor<CostInput> = InputDescriptor {
    fields: &RESOURCE_FIELDS,
    ..OVERVIEW_INPUT
};

const BALANCES_INPUT: InputDescriptor<()> = InputDescriptor {
    defaults: || (),
    fields: &[],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: None,
};

pub(super) const GET_COST_OVERVIEW: Operation = Operation {
    name: "get-cost-overview",
    sdk_operation: "billing.GetCostOverview",
    short_help: "Get cost overview",
    example: Some(
        "greennode billing get-cost-overview --start-date <start-date> --end-date <end-date>",
    ),
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Read,
    flags: &flags(&OVERVIEW_FIELDS),
    json_fields: &[],
    extra_flags: &[],
    notes: &[],
    handler: Some(Handler::new(
        &OVERVIEW_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management(
                invocation,
                operation,
                matches,
                &OVERVIEW_INPUT,
                get_cost_overview,
            ))
        },
    )),
};

pub(super) const LIST_COST_RESOURCES: Operation = Operation {
    name: "list-cost-resources",
    sdk_operation: "billing.ListCostResources",
    short_help: "List cost resources",
    example: Some(
        "greennode billing list-cost-resources --start-date <start-date> --end-date <end-date>",
    ),
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Read,
    flags: &flags(&RESOURCE_FIELDS),
    json_fields: &[],
    extra_flags: &[],
    notes: &[],
    handler: Some(Handler::new(
        &RESOURCES_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management(
                invocation,
                operation,
                matches,
                &RESOURCES_INPUT,
                list_cost_resources,
            ))
        },
    )),
};

pub(super) const GET_BALANCES: Operation = Operation {
    name: "get-balances",
    sdk_operation: "billing.GetBalances",
    short_help: "Get account balances",
    example: Some("greennode billing get-balances --query Balances"),
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Read,
    flags: &[],
    json_fields: &[],
    extra_flags: &[],
    notes: &[],
    handler: Some(Handler::new(
        &BALANCES_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management(
                invocation,
                operation,
                matches,
                &BALANCES_INPUT,
                get_balances,
            ))
        },
    )),
};

fn get_cost_overview<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    staged: &'a CostInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let input = GetCostOverviewInput {
            product: staged.product.clone(),
            resource_type: staged.resource_type.clone(),
            resource_id: staged.resource_id.clone(),
            query: staged.query.clone(),
            group_by: staged.group_by.clone(),
            interval: staged.interval.clone(),
            ..GetCostOverviewInput::new(&staged.start_date, &staged.end_date)
        };
        let result = Client::new(config).get_cost_overview(ctx, input).await;
        dispatch::project_result(result, std::convert::identity)
    })
}

fn list_cost_resources<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    staged: &'a CostInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let input = ListCostResourcesInput {
            product: staged.product.clone(),
            resource_type: staged.resource_type.clone(),
            resource_id: staged.resource_id.clone(),
            query: staged.query.clone(),
            page: staged.page,
            size: staged.size,
            sort: staged.sort.clone(),
            order: staged.order.clone(),
            ..ListCostResourcesInput::new(&staged.start_date, &staged.end_date)
        };
        let result = Client::new(config).list_cost_resources(ctx, input).await;
        dispatch::project_result(result, std::convert::identity)
    })
}

fn get_balances<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    _: &'a (),
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let result = Client::new(config)
            .get_balances(ctx, GetBalancesInput {})
            .await;
        dispatch::project_result(result, std::convert::identity)
    })
}
