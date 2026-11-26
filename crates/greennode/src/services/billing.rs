mod budget_reads;

use clap::ArgMatches;
use greennode_sdk::billing::{Client, GetBudgetInput, ListBudgetsInput};
use greennode_sdk::{Config, RequestContext};

use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, Service};

const STATUS: Field = Field {
    name: "status",
    go_type: "string",
    required: false,
};

const BUDGET_UUID: Field = Field {
    name: "budget-uuid",
    go_type: "string",
    required: true,
};

fn check_input_source(_: &ArgMatches, json: Option<&str>) -> Result<(), InputError> {
    if json == Some("file://") {
        return Err(InputError("--cli-input-json: cannot open file"));
    }
    Ok(())
}

fn json_string(value: serde_json::Value) -> Result<String, ()> {
    match value {
        serde_json::Value::String(value) => Ok(value),
        _ => Err(()),
    }
}

#[derive(Default)]
struct ListInput {
    status: String,
}

const LIST_INPUT: InputDescriptor<ListInput> = InputDescriptor {
    defaults: ListInput::default,
    fields: &[InputField {
        json_name: "Status",
        flag: Some(STATUS),
        secret: false,
        nullable: false,
        set_json: |input, value| {
            input.status = json_string(value)?;
            Ok(())
        },
        set_flag: Some(|input, matches| {
            input.status = matches.get_one::<String>(STATUS.name).ok_or(())?.clone();
            Ok(())
        }),
        requirement: Requirement::Optional,
    }],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: None,
};

#[derive(Default)]
struct GetInput {
    budget_uuid: String,
}

const GET_INPUT: InputDescriptor<GetInput> = InputDescriptor {
    defaults: GetInput::default,
    fields: &[InputField {
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
    }],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: None,
};

pub(super) const SERVICE: Service = Service {
    name: "billing",
    operations: &[
        Operation {
            name: "list-budgets",
            sdk_operation: "billing.ListBudgets",
            short_help: "List budgets",
            example: None,
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[STATUS],
            json_fields: &[],
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
                        list_budgets,
                    ))
                },
            )),
        },
        Operation {
            name: "get-budget",
            sdk_operation: "billing.GetBudget",
            short_help: "Get a budget",
            example: Some(
                "greennode billing get-budget --budget-uuid <budget-uuid> --query Budget",
            ),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[BUDGET_UUID],
            json_fields: &[],
            extra_flags: &[],
            notes: &[],
            handler: Some(Handler::new(
                &GET_INPUT,
                |operation, matches, invocation| {
                    Box::pin(dispatch::management(
                        invocation, operation, matches, &GET_INPUT, get_budget,
                    ))
                },
            )),
        },
        budget_reads::GET_CURRENT_PERIOD_COST,
        budget_reads::LIST_BUDGET_THRESHOLDS,
        budget_reads::LIST_BUDGET_ALERTS,
    ],
};

fn list_budgets<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    staged: &'a ListInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let input = ListBudgetsInput {
            status: staged.status.clone(),
        };
        let result = Client::new(config).list_budgets(ctx, input).await;
        dispatch::project_result(result, std::convert::identity)
    })
}

fn get_budget<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    staged: &'a GetInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let input = GetBudgetInput::new(&staged.budget_uuid);
        let result = Client::new(config).get_budget(ctx, input).await;
        dispatch::project_result(result, std::convert::identity)
    })
}
