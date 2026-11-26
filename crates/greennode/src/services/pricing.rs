mod estimate;

use crate::cli::Invocation;
use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::errors::{Rule, RuleExit, RuleName, RuleOperation};
use crate::cli::input::{InputDescriptor, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, SelectedOperation, Service};
use clap::ArgMatches;
use greennode_sdk::pricing::{Client, GetQuoteInput};
use greennode_sdk::{Config, ErrorClass, RequestContext};

const RESOURCE_TYPE: Field = Field {
    name: "resource-type",
    go_type: "string",
    required: true,
};
const ACTION: Field = Field {
    name: "action",
    go_type: "string",
    required: false,
};
const RESOURCE_INFO: Field = Field {
    name: "ResourceInfo",
    go_type: "map[string]any",
    required: false,
};

const GET_INPUT: InputDescriptor<GetQuoteInput> = InputDescriptor {
    defaults: || GetQuoteInput::new(""),
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: None,
    guard: None,
    fields: &[
        InputField {
            json_name: "ResourceType",
            flag: Some(RESOURCE_TYPE),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.resource_type = value.as_str().ok_or(())?.into();
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.resource_type = matches
                    .get_one::<String>("resource-type")
                    .ok_or(())?
                    .clone();
                Ok(())
            }),
            requirement: Requirement::Nonzero(|input| !input.resource_type.is_empty()),
        },
        InputField {
            json_name: "Action",
            flag: Some(ACTION),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.action = value.as_str().ok_or(())?.into();
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.action = matches.get_one::<String>("action").ok_or(())?.clone();
                Ok(())
            }),
            requirement: Requirement::Optional,
        },
        InputField {
            json_name: "ResourceInfo",
            flag: None,
            secret: false,
            nullable: true,
            set_json: |input, value| {
                input.resource_info = serde_json::from_value(value).map_err(|_| ())?;
                Ok(())
            },
            set_flag: None,
            requirement: Requirement::Optional,
        },
    ],
};

const GET_QUOTE: Operation = Operation {
    name: "get-quote",
    sdk_operation: "pricing.GetQuote",
    short_help: "Get a resource price quote",
    example: None,
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Read,
    flags: &[RESOURCE_TYPE, ACTION],
    json_fields: &[RESOURCE_INFO],
    extra_flags: &[],
    notes: &[
        "Prices are VND totals that include VAT. The API gives no VAT breakdown. For vMonitor, use the monitor service's quote commands.",
        "Empty `--action` sends `create`. Only exact `--action resize` accepts a negative `OptimumPrice`. A negative amount is the expected credit in VND for the remaining period. Zero is a valid quote.",
        "Responses are limited to 1 MiB. Quote requests refuse redirects. Malformed successful replies return `InvalidResponse` with the observed HTTP status.",
    ],
    handler: Some(Handler::new(&GET_INPUT, get_quote)),
};

fn get_quote<'a>(
    selected: SelectedOperation,
    matches: &'a ArgMatches,
    invocation: &'a mut Invocation<'_>,
) -> FutureResult<'a, u8> {
    Box::pin(dispatch::management(
        invocation, selected, matches, &GET_INPUT, run_quote,
    ))
}

fn run_quote<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a GetQuoteInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            Client::new(config).get_quote(ctx, input.clone()).await,
            |output| output,
        )
    })
}

pub(super) const SERVICE: Service = Service {
    name: "pricing",
    operations: &[GET_QUOTE, estimate::ESTIMATE],
    error_rules: &[
        Rule {
            identity: "pricing-discovery-incomplete",
            matches: |error| error.has_class(ErrorClass::DiscoveryIncomplete),
            name: Some(RuleName {
                rank: 1,
                code: "DiscoveryIncomplete",
                operation: RuleOperation::Omit,
                include_api_status: false,
            }),
            exit: Some(RuleExit::Failure),
            message: Some("pricing discovery incomplete"),
        },
        Rule {
            identity: "estimate-limit",
            matches: |error| error.has_class(ErrorClass::EstimateLimit),
            name: Some(RuleName {
                rank: 2,
                code: "EstimateLimit",
                operation: RuleOperation::Omit,
                include_api_status: false,
            }),
            exit: Some(RuleExit::Failure),
            message: None,
        },
        Rule {
            identity: "estimate-overflow",
            matches: |error| error.has_class(ErrorClass::EstimateOverflow),
            name: Some(RuleName {
                rank: 3,
                code: "EstimateOverflow",
                operation: RuleOperation::Omit,
                include_api_status: false,
            }),
            exit: Some(RuleExit::Failure),
            message: Some("estimate arithmetic overflow"),
        },
        Rule {
            identity: "pricing-invalid-input",
            matches: |error| {
                error.has_class(ErrorClass::InvalidInput)
                    && !error.has_class(ErrorClass::DiscoveryIncomplete)
            },
            name: Some(RuleName {
                rank: 4,
                code: "InvalidUsage",
                operation: RuleOperation::Omit,
                include_api_status: false,
            }),
            exit: Some(RuleExit::Usage),
            message: None,
        },
    ],
};
