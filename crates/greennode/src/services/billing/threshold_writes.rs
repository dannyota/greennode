use super::{BUDGET_UUID, check_input_source, json_string};
use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};
use greennode_sdk::billing::{
    Client, CreateBudgetThresholdInput, DeleteBudgetThresholdInput, UpdateBudgetThresholdInput,
};
use greennode_sdk::{Config, RequestContext};

const THRESHOLD_UUID: Field = Field {
    name: "threshold-uuid",
    ..BUDGET_UUID
};
const THRESHOLD_TYPE: Field = Field {
    name: "threshold-type",
    ..BUDGET_UUID
};
const PERCENTAGE: Field = Field {
    name: "threshold-percentage",
    go_type: "int",
    required: true,
};
const MAX_ALERTS: Field = Field {
    name: "max-alerts-per-period",
    required: false,
    ..PERCENTAGE
};
const REMINDER_HOURS: Field = Field {
    name: "reminder-interval-hours",
    ..MAX_ALERTS
};
const UPDATE_PERCENTAGE: Field = Field {
    go_type: "*int",
    required: false,
    ..PERCENTAGE
};
const UPDATE_MAX_ALERTS: Field = Field {
    name: MAX_ALERTS.name,
    ..UPDATE_PERCENTAGE
};
const UPDATE_REMINDER_HOURS: Field = Field {
    name: REMINDER_HOURS.name,
    ..UPDATE_PERCENTAGE
};
const ENABLED: Field = Field {
    name: "enabled",
    go_type: "*bool",
    required: false,
};

#[derive(Default)]
struct ThresholdInput {
    budget_uuid: Option<String>,
    threshold_uuid: Option<String>,
    threshold_type: Option<String>,
    threshold_percentage: Option<i64>,
    enabled: Option<bool>,
    max_alerts_per_period: Option<i64>,
    reminder_interval_hours: Option<i64>,
}

macro_rules! input_field {
    ($json:literal, $flag:ident, $field:ident, $ty:ty, $parse:expr, $required:expr) => {
        InputField {
            json_name: $json,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$field = Some(($parse)(value)?);
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$field = matches.get_one::<$ty>($flag.name).cloned();
                Ok(())
            }),
            requirement: $required,
        }
    };
}

const BUDGET_ID: InputField<ThresholdInput> = input_field!(
    "BudgetUUID",
    BUDGET_UUID,
    budget_uuid,
    String,
    json_string,
    Requirement::Nonzero(|input| input.budget_uuid.as_ref().is_some_and(|id| !id.is_empty()))
);
const THRESHOLD_ID: InputField<ThresholdInput> = input_field!(
    "ThresholdUUID",
    THRESHOLD_UUID,
    threshold_uuid,
    String,
    json_string,
    Requirement::Nonzero(|input| input
        .threshold_uuid
        .as_ref()
        .is_some_and(|id| !id.is_empty()))
);

fn json_int(value: serde_json::Value) -> Result<i64, ()> {
    value.as_i64().ok_or(())
}

macro_rules! int_field {
    ($json:literal, $flag:ident, $field:ident) => {
        input_field!($json, $flag, $field, i64, json_int, Requirement::Optional)
    };
}

const CREATE_INPUT: InputDescriptor<ThresholdInput> = InputDescriptor {
    defaults: ThresholdInput::default,
    fields: &[
        BUDGET_ID,
        input_field!(
            "ThresholdType",
            THRESHOLD_TYPE,
            threshold_type,
            String,
            json_string,
            Requirement::Nonzero(|input| input
                .threshold_type
                .as_ref()
                .is_some_and(|kind| !kind.is_empty()))
        ),
        input_field!(
            "ThresholdPercentage",
            PERCENTAGE,
            threshold_percentage,
            i64,
            json_int,
            Requirement::Nonzero(|input| input
                .threshold_percentage
                .is_some_and(|value| value != 0))
        ),
        int_field!("MaxAlertsPerPeriod", MAX_ALERTS, max_alerts_per_period),
        int_field!(
            "ReminderIntervalHours",
            REMINDER_HOURS,
            reminder_interval_hours
        ),
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: None,
};

fn check_threshold_uuid(input: &ThresholdInput, _: &clap::ArgMatches) -> Result<(), InputError> {
    if input.threshold_uuid.as_ref().is_some_and(|id| {
        !id.is_empty() && !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    }) {
        return Err(InputError(
            "--threshold-uuid must contain only ASCII letters, digits, and hyphens",
        ));
    }
    Ok(())
}

const UPDATE_INPUT: InputDescriptor<ThresholdInput> = InputDescriptor {
    fields: &[
        BUDGET_ID,
        THRESHOLD_ID,
        int_field!(
            "ThresholdPercentage",
            UPDATE_PERCENTAGE,
            threshold_percentage
        ),
        input_field!(
            "Enabled",
            ENABLED,
            enabled,
            bool,
            |value: serde_json::Value| value.as_bool().ok_or(()),
            Requirement::Optional
        ),
        int_field!(
            "MaxAlertsPerPeriod",
            UPDATE_MAX_ALERTS,
            max_alerts_per_period
        ),
        int_field!(
            "ReminderIntervalHours",
            UPDATE_REMINDER_HOURS,
            reminder_interval_hours
        ),
    ],
    guard: Some(check_threshold_uuid),
    ..CREATE_INPUT
};

const DELETE_INPUT: InputDescriptor<ThresholdInput> = InputDescriptor {
    fields: &[BUDGET_ID, THRESHOLD_ID],
    guard: Some(check_threshold_uuid),
    ..CREATE_INPUT
};

const WRITE_NOTE: &str = "Threshold writes are sent once and never follow redirects. Responses are limited\nto 8 MiB. If the outcome is unconfirmed, list the budget thresholds before\nretrying. Malformed successful replies return InvalidResponse with the observed\nHTTP status.";

pub(super) const CREATE_BUDGET_THRESHOLD: Operation = Operation {
    name: "create-budget-threshold",
    sdk_operation: "billing.CreateBudgetThreshold",
    short_help: "Create a budget threshold",
    example: None,
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Write,
    flags: &[
        BUDGET_UUID,
        THRESHOLD_TYPE,
        PERCENTAGE,
        MAX_ALERTS,
        REMINDER_HOURS,
    ],
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
                create_budget_threshold,
            ))
        },
    )),
};

pub(super) const UPDATE_BUDGET_THRESHOLD: Operation = Operation {
    name: "update-budget-threshold",
    sdk_operation: "billing.UpdateBudgetThreshold",
    short_help: "Update a budget threshold",
    example: Some(
        "greennode billing update-budget-threshold --budget-uuid <budget-uuid> --threshold-uuid <threshold-uuid> --enabled=false",
    ),
    flags: &[
        BUDGET_UUID,
        THRESHOLD_UUID,
        UPDATE_PERCENTAGE,
        ENABLED,
        UPDATE_MAX_ALERTS,
        UPDATE_REMINDER_HOURS,
    ],
    handler: Some(Handler::new(
        &UPDATE_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management(
                invocation,
                operation,
                matches,
                &UPDATE_INPUT,
                update_budget_threshold,
            ))
        },
    )),
    ..CREATE_BUDGET_THRESHOLD
};

pub(super) const DELETE_BUDGET_THRESHOLD: Operation = Operation {
    name: "delete-budget-threshold",
    sdk_operation: "billing.DeleteBudgetThreshold",
    short_help: "Delete a budget threshold",
    kind: Kind::DestructiveWrite,
    flags: &[BUDGET_UUID, THRESHOLD_UUID],
    handler: Some(Handler::new(
        &DELETE_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management(
                invocation,
                operation,
                matches,
                &DELETE_INPUT,
                delete_budget_threshold,
            ))
        },
    )),
    ..CREATE_BUDGET_THRESHOLD
};

fn create_budget_threshold<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    staged: &'a ThresholdInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let mut input = CreateBudgetThresholdInput::new(
            staged.budget_uuid.as_deref().unwrap_or_default(),
            staged.threshold_type.as_deref().unwrap_or_default(),
            staged.threshold_percentage.unwrap_or_default(),
        );
        input.max_alerts_per_period = staged.max_alerts_per_period.unwrap_or_default();
        input.reminder_interval_hours = staged.reminder_interval_hours.unwrap_or_default();
        let result = Client::new(config)
            .create_budget_threshold(ctx, input)
            .await;
        dispatch::project_result(result, std::convert::identity)
    })
}

fn update_budget_threshold<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    staged: &'a ThresholdInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let input = UpdateBudgetThresholdInput {
            budget_uuid: staged.budget_uuid.clone().unwrap_or_default(),
            threshold_uuid: staged.threshold_uuid.clone().unwrap_or_default(),
            threshold_percentage: staged.threshold_percentage,
            enabled: staged.enabled,
            max_alerts_per_period: staged.max_alerts_per_period,
            reminder_interval_hours: staged.reminder_interval_hours,
        };
        let result = Client::new(config)
            .update_budget_threshold(ctx, input)
            .await;
        dispatch::project_result(result, std::convert::identity)
    })
}

fn delete_budget_threshold<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    staged: &'a ThresholdInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let input = DeleteBudgetThresholdInput::new(
            staged.budget_uuid.as_deref().unwrap_or_default(),
            staged.threshold_uuid.as_deref().unwrap_or_default(),
        );
        let result = Client::new(config)
            .delete_budget_threshold(ctx, input)
            .await;
        dispatch::project_result(result, std::convert::identity)
    })
}
