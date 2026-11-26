use super::{BUDGET_UUID, GET_INPUT, GetInput, check_input_source, json_string};
use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};
use greennode_sdk::billing::{
    Client, GetCurrentPeriodCostInput, ListBudgetAlertsInput, ListBudgetThresholdsInput,
    PeriodCost, Threshold,
};
use greennode_sdk::{Config, List, RequestContext};
use serde::Serialize;

const PERIOD_KEY: Field = Field {
    name: "period-key",
    go_type: "string",
    required: false,
};

const COST_INPUT: InputDescriptor<()> = InputDescriptor {
    defaults: || (),
    fields: &[],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: None,
};

#[derive(Default)]
struct AlertInput {
    budget_uuid: String,
    period_key: String,
}

const ALERT_INPUT: InputDescriptor<AlertInput> = InputDescriptor {
    defaults: AlertInput::default,
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
        InputField {
            json_name: "PeriodKey",
            flag: Some(PERIOD_KEY),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.period_key = json_string(value)?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.period_key = matches
                    .get_one::<String>(PERIOD_KEY.name)
                    .ok_or(())?
                    .clone();
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

pub(super) const GET_CURRENT_PERIOD_COST: Operation = Operation {
    name: "get-current-period-cost",
    sdk_operation: "billing.GetCurrentPeriodCost",
    short_help: "Get current period cost",
    example: Some("greennode billing get-current-period-cost --query PeriodCost"),
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Read,
    flags: &[],
    json_fields: &[],
    extra_flags: &[],
    notes: &[],
    handler: Some(Handler::new(
        &COST_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management(
                invocation,
                operation,
                matches,
                &COST_INPUT,
                get_current_period_cost,
            ))
        },
    )),
};

pub(super) const LIST_BUDGET_THRESHOLDS: Operation = Operation {
    name: "list-budget-thresholds",
    sdk_operation: "billing.ListBudgetThresholds",
    short_help: "List budget thresholds",
    example: None,
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
                invocation,
                operation,
                matches,
                &GET_INPUT,
                list_budget_thresholds,
            ))
        },
    )),
};

pub(super) const LIST_BUDGET_ALERTS: Operation = Operation {
    name: "list-budget-alerts",
    sdk_operation: "billing.ListBudgetAlerts",
    short_help: "List budget alerts",
    example: None,
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Read,
    flags: &[BUDGET_UUID, PERIOD_KEY],
    json_fields: &[],
    extra_flags: &[],
    notes: &[],
    handler: Some(Handler::new(
        &ALERT_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management(
                invocation,
                operation,
                matches,
                &ALERT_INPUT,
                list_budget_alerts,
            ))
        },
    )),
};

fn get_current_period_cost<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    _staged: &'a (),
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let result = Client::new(config)
            .get_current_period_cost(ctx, GetCurrentPeriodCostInput::default())
            .await;
        dispatch::project_result(result, |output| PeriodCostOutputView {
            period_cost: PeriodCostView(output.period_cost),
        })
    })
}

fn list_budget_thresholds<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    staged: &'a GetInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let input = ListBudgetThresholdsInput::new(&staged.budget_uuid);
        let result = Client::new(config).list_budget_thresholds(ctx, input).await;
        dispatch::project_result(result, |output| List {
            items: output.items.into_iter().map(ThresholdView).collect(),
        })
    })
}

fn list_budget_alerts<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    staged: &'a AlertInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let input = ListBudgetAlertsInput {
            budget_uuid: staged.budget_uuid.clone(),
            period_key: staged.period_key.clone(),
        };
        let result = Client::new(config).list_budget_alerts(ctx, input).await;
        dispatch::project_result(result, std::convert::identity)
    })
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct PeriodCostOutputView {
    period_cost: PeriodCostView,
}

#[derive(Serialize)]
struct PeriodCostView(#[serde(with = "PeriodCostFields")] PeriodCost);

#[derive(Serialize)]
#[serde(remote = "PeriodCost", rename_all = "PascalCase")]
struct PeriodCostFields {
    period_key: String,
    period_start: String,
    period_end: String,
    actual_cost: f64,
    forecasted_cost: f64,
}

#[derive(Serialize)]
struct ThresholdView(#[serde(with = "ThresholdFields")] Threshold);

#[derive(Serialize)]
#[serde(remote = "Threshold", rename_all = "PascalCase")]
struct ThresholdFields {
    #[serde(rename = "UUID")]
    uuid: String,
    threshold_type: String,
    threshold_percentage: f64,
    comparison_operator: String,
    enabled: bool,
    max_alerts_per_period: i64,
    reminder_interval_hours: i64,
    notification_state: String,
    last_alert_at: String,
}
