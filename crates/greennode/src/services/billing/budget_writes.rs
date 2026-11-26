use super::{BUDGET_UUID, GET_INPUT, GetInput, STATUS, check_input_source, json_string};
use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};
use greennode_sdk::billing::{Client, CreateBudgetInput, DeleteBudgetInput, UpdateBudgetInput};
use greennode_sdk::{Config, RequestContext};

const NAME: Field = Field {
    name: "name",
    go_type: "string",
    required: true,
};
const PERIOD_TYPE: Field = Field {
    name: "period-type",
    ..NAME
};
const TYPE: Field = Field {
    name: "type",
    ..NAME
};
const LIMIT_AMOUNT: Field = Field {
    name: "limit-amount",
    go_type: "int64",
    required: true,
};
const UPDATE_NAME: Field = Field {
    go_type: "*string",
    required: false,
    ..NAME
};
const UPDATE_PERIOD_TYPE: Field = Field {
    name: PERIOD_TYPE.name,
    ..UPDATE_NAME
};
const UPDATE_TYPE: Field = Field {
    name: TYPE.name,
    ..UPDATE_NAME
};
const UPDATE_STATUS: Field = Field {
    name: STATUS.name,
    ..UPDATE_NAME
};
const UPDATE_LIMIT_AMOUNT: Field = Field {
    go_type: "*int64",
    required: false,
    ..LIMIT_AMOUNT
};

#[derive(Default)]
struct BudgetInput {
    budget_uuid: String,
    name: Option<String>,
    period_type: Option<String>,
    budget_type: Option<String>,
    limit_amount: Option<i64>,
    status: Option<String>,
}

macro_rules! string_field {
    ($json:literal, $flag:ident, $field:ident) => {
        InputField {
            json_name: $json,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$field = Some(json_string(value)?);
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$field = Some(matches.get_one::<String>($flag.name).ok_or(())?.clone());
                Ok(())
            }),
            requirement: if $flag.required {
                Requirement::Nonzero(|input| {
                    input
                        .$field
                        .as_deref()
                        .is_some_and(|value| !value.is_empty())
                })
            } else {
                Requirement::Optional
            },
        }
    };
}

const fn limit_amount(flag: Field) -> InputField<BudgetInput> {
    InputField {
        json_name: "LimitAmount",
        flag: Some(flag),
        secret: false,
        nullable: false,
        set_json: |input, value| {
            input.limit_amount = Some(value.as_i64().ok_or(())?);
            Ok(())
        },
        set_flag: Some(|input, matches| {
            input.limit_amount = Some(*matches.get_one::<i64>(LIMIT_AMOUNT.name).ok_or(())?);
            Ok(())
        }),
        requirement: if flag.required {
            Requirement::Nonzero(|input| input.limit_amount.is_some_and(|value| value != 0))
        } else {
            Requirement::Optional
        },
    }
}

const CREATE_INPUT: InputDescriptor<BudgetInput> = InputDescriptor {
    defaults: BudgetInput::default,
    fields: &[
        string_field!("Name", NAME, name),
        string_field!("PeriodType", PERIOD_TYPE, period_type),
        string_field!("Type", TYPE, budget_type),
        limit_amount(LIMIT_AMOUNT),
        string_field!("Status", STATUS, status),
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: None,
};

const UPDATE_INPUT: InputDescriptor<BudgetInput> = InputDescriptor {
    fields: &[
        InputField {
            json_name: "BudgetUUID",
            flag: Some(BUDGET_UUID),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.budget_uuid = json_string(value)?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.budget_uuid = matches
                    .get_one::<String>(BUDGET_UUID.name)
                    .ok_or(())?
                    .clone();
                Ok(())
            }),
            requirement: Requirement::Nonzero(|input| !input.budget_uuid.is_empty()),
        },
        string_field!("Name", UPDATE_NAME, name),
        string_field!("PeriodType", UPDATE_PERIOD_TYPE, period_type),
        string_field!("Type", UPDATE_TYPE, budget_type),
        limit_amount(UPDATE_LIMIT_AMOUNT),
        string_field!("Status", UPDATE_STATUS, status),
    ],
    ..CREATE_INPUT
};

const WRITE_NOTE: &str = "Budget writes are sent once and never follow redirects. Responses are limited to\n8 MiB. If the outcome is unconfirmed, read current budgets before retrying.";

pub(super) const CREATE_BUDGET: Operation = Operation {
    name: "create-budget",
    sdk_operation: "billing.CreateBudget",
    short_help: "Create a budget",
    example: None,
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Write,
    flags: &[NAME, PERIOD_TYPE, TYPE, LIMIT_AMOUNT, STATUS],
    json_fields: &[],
    extra_flags: &[],
    notes: &[WRITE_NOTE],
    handler: Some(Handler::new(
        &CREATE_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management(
                invocation,
                operation,
                matches,
                &CREATE_INPUT,
                create_budget,
            ))
        },
    )),
};

pub(super) const UPDATE_BUDGET: Operation = Operation {
    name: "update-budget",
    sdk_operation: "billing.UpdateBudget",
    short_help: "Update a budget",
    example: Some("greennode billing update-budget --budget-uuid <budget-uuid> --status PAUSED"),
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Write,
    flags: &[
        BUDGET_UUID,
        UPDATE_NAME,
        UPDATE_PERIOD_TYPE,
        UPDATE_TYPE,
        UPDATE_LIMIT_AMOUNT,
        UPDATE_STATUS,
    ],
    json_fields: &[],
    extra_flags: &[],
    notes: &[WRITE_NOTE],
    handler: Some(Handler::new(
        &UPDATE_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management(
                invocation,
                operation,
                matches,
                &UPDATE_INPUT,
                update_budget,
            ))
        },
    )),
};

pub(super) const DELETE_BUDGET: Operation = Operation {
    name: "delete-budget",
    sdk_operation: "billing.DeleteBudget",
    short_help: "Delete a budget and its thresholds",
    example: None,
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::DestructiveWrite,
    flags: &[BUDGET_UUID],
    json_fields: &[],
    extra_flags: &[],
    notes: &[WRITE_NOTE],
    handler: Some(Handler::new(
        &GET_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management(
                invocation,
                operation,
                matches,
                &GET_INPUT,
                delete_budget,
            ))
        },
    )),
};

fn create_budget<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    staged: &'a BudgetInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let mut input = CreateBudgetInput::new(
            staged.name.as_deref().unwrap_or_default(),
            staged.period_type.as_deref().unwrap_or_default(),
            staged.budget_type.as_deref().unwrap_or_default(),
            staged.limit_amount.unwrap_or_default(),
        );
        input.status = staged.status.clone().unwrap_or_default();
        let result = Client::new(config).create_budget(ctx, input).await;
        dispatch::project_result(result, std::convert::identity)
    })
}

fn update_budget<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    staged: &'a BudgetInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let input = UpdateBudgetInput {
            budget_uuid: staged.budget_uuid.clone(),
            name: staged.name.clone(),
            period_type: staged.period_type.clone(),
            r#type: staged.budget_type.clone(),
            limit_amount: staged.limit_amount,
            status: staged.status.clone(),
        };
        let result = Client::new(config).update_budget(ctx, input).await;
        dispatch::project_result(result, std::convert::identity)
    })
}

fn delete_budget<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    staged: &'a GetInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let input = DeleteBudgetInput::new(&staged.budget_uuid);
        let result = Client::new(config).delete_budget(ctx, input).await;
        dispatch::project_result(result, std::convert::identity)
    })
}
