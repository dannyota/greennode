use greennode_sdk::billing::{
    Client, ListPaymentsInput, ListResourcesInput, ListTransactionsInput,
};
use greennode_sdk::{Config, RequestContext};

use super::{check_input_source, json_string};
use crate::cli::dispatch::{self, ExecutionResult, FutureResult, OutputView};
use crate::cli::input::{InputDescriptor, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};
use crate::cli::render::TableView;

const START_DATE: Field = Field {
    name: "start-date",
    go_type: "string",
    required: true,
};
const END_DATE: Field = Field {
    name: "end-date",
    go_type: "string",
    required: false,
};
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
const ALL: Field = Field {
    name: "all",
    go_type: "bool",
    required: false,
};

const HISTORY_INPUT: InputDescriptor<ListTransactionsInput> = InputDescriptor {
    defaults: || ListTransactionsInput::new(""),
    fields: &[
        InputField {
            json_name: "StartDate",
            flag: Some(START_DATE),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.start_date = json_string(value)?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.start_date = matches
                    .get_one::<String>(START_DATE.name)
                    .ok_or(())?
                    .clone();
                Ok(())
            }),
            requirement: Requirement::Nonzero(|input| !input.start_date.is_empty()),
        },
        InputField {
            json_name: "EndDate",
            flag: Some(END_DATE),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.end_date = json_string(value)?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.end_date = matches.get_one::<String>(END_DATE.name).ok_or(())?.clone();
                Ok(())
            }),
            requirement: Requirement::Optional,
        },
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
        InputField {
            json_name: "All",
            flag: Some(ALL),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.all = value.as_bool().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.all = *matches.get_one::<bool>(ALL.name).ok_or(())?;
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

const RESOURCES_INPUT: InputDescriptor<ListResourcesInput> = InputDescriptor {
    defaults: ListResourcesInput::default,
    fields: &[],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: None,
};

const HISTORY_DATES: &str = "Dates are inclusive YYYY-MM-DD in fixed UTC+07:00. An empty end date means\ntoday. Size defaults to 50 and is capped at 50. `--all` starts at page 1 and\nrejects `--page` above 1.";
const HISTORY_OUTPUT: &str = "JSON, text, and explicit queries can contain account data. Queries use `Items`,\n`Page`, `PageSize`, `TotalPage`, and `TotalItem`.";
const HISTORY_LIMITS: &str = "A complete walk is limited to 100 pages, 8 MiB per response, 32 MiB of JSON\nvalues, and two minutes. A walk limit or changing coverage can return\n`HistoryIncomplete` with no output.";
const WITHHOLD: &str = "billing history query failed; result withheld";

pub(super) const LIST_TRANSACTIONS: Operation = Operation {
    name: "list-transactions",
    sdk_operation: "billing.ListTransactions",
    short_help: "Read wallet transactions. Output can contain account data.",
    example: Some(
        "greennode billing list-transactions --start-date 2024-11-01 --end-date 2024-11-30 --all",
    ),
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Read,
    flags: &[START_DATE, END_DATE, PAGE, SIZE, ALL],
    json_fields: &[],
    extra_flags: &[],
    notes: &[
        HISTORY_DATES,
        HISTORY_OUTPUT,
        HISTORY_LIMITS,
        "Default table columns are CreatedAt, Type, TypeCode, Direction, Amount, and\nBalance.",
    ],
    handler: Some(Handler::new(
        &HISTORY_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management_with_view(
                invocation,
                operation,
                matches,
                &HISTORY_INPUT,
                list_transactions,
                OutputView {
                    custom: None,
                    table: Some(TableView {
                        columns: &[
                            "CreatedAt",
                            "Type",
                            "TypeCode",
                            "Direction",
                            "Amount",
                            "Balance",
                        ],
                        show_empty_headers: true,
                    }),
                    withhold: Some(WITHHOLD),
                },
            ))
        },
    )),
};

pub(super) const LIST_PAYMENTS: Operation = Operation {
    name: "list-payments",
    sdk_operation: "billing.ListPayments",
    short_help: "Read payment history. Output can contain account data.",
    example: Some("greennode billing list-payments --start-date 2024-11-01 --end-date 2024-11-30"),
    notes: &[
        HISTORY_DATES,
        HISTORY_OUTPUT,
        HISTORY_LIMITS,
        "Default table columns are CreatedAt, Gateway, Status, TypeCode, and Amount.",
    ],
    handler: Some(Handler::new(
        &HISTORY_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management_with_view(
                invocation,
                operation,
                matches,
                &HISTORY_INPUT,
                list_payments,
                OutputView {
                    custom: None,
                    table: Some(TableView {
                        columns: &["CreatedAt", "Gateway", "Status", "TypeCode", "Amount"],
                        show_empty_headers: true,
                    }),
                    withhold: Some(WITHHOLD),
                },
            ))
        },
    )),
    ..LIST_TRANSACTIONS
};

pub(super) const LIST_RESOURCES: Operation = Operation {
    name: "list-resources",
    sdk_operation: "billing.ListResources",
    short_help: "List prepaid resource renewal state",
    example: None,
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Read,
    flags: &[],
    json_fields: &[],
    extra_flags: &[],
    notes: &[
        "Lists prepaid resource renewal state across products. Tables show billing\ntimestamps in epoch milliseconds. The currency and meaning of `Cost` are\nunverified.",
    ],
    handler: Some(Handler::new(
        &RESOURCES_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management_with_view(
                invocation,
                operation,
                matches,
                &RESOURCES_INPUT,
                list_resources,
                OutputView {
                    custom: None,
                    table: Some(TableView {
                        columns: &[
                            "ArtifactID",
                            "ArtifactName",
                            "ArtifactType",
                            "BillingType",
                            "EndBillingTime",
                            "IsRenewing",
                            "Product",
                            "RenewPeriod",
                            "RenewType",
                            "StartBillingTime",
                        ],
                        show_empty_headers: false,
                    }),
                    withhold: None,
                },
            ))
        },
    )),
};

fn list_transactions<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a ListTransactionsInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            Client::new(config)
                .list_transactions(ctx, input.clone())
                .await,
            |output| output,
        )
    })
}

fn list_payments<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a ListTransactionsInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let input = ListPaymentsInput {
            start_date: input.start_date.clone(),
            end_date: input.end_date.clone(),
            page: input.page,
            size: input.size,
            all: input.all,
        };
        dispatch::project_result(
            Client::new(config).list_payments(ctx, input).await,
            |output| output,
        )
    })
}

fn list_resources<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a ListResourcesInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            Client::new(config).list_resources(ctx, *input).await,
            |output| output,
        )
    })
}
