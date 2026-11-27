mod outcome;

use super::*;
use crate::cli::{dispatch::WritePolicy, op::ParsePreflight};
use greennode_sdk::monitor as sdk;

const DAYS: InputField<Input> = field!("max-days-left", "MaxDaysLeft", max_days_left, "int", i64);
const NO_WAIT: InputField<Input> = field!("no-wait", "NoWait", no_wait, "bool", bool);
const ID: InputField<Input> = field!(
    "expected-quota-id",
    "ExpectedQuotaID",
    expected_quota_id,
    "string",
    String
);
const REQUIRED_ID: InputField<Input> = field!(
    "expected-quota-id",
    "ExpectedQuotaID",
    expected_quota_id,
    required
);

const CREATE_FIELDS: &[InputField<Input>] = &[CLASS_ID, PACKAGE_ID, QUANTITY, CAP, NO_WAIT];
const RENEW_FIELDS: &[InputField<Input>] = &[EXPECTED_END_TIME, CAP, DAYS, NO_WAIT];
const RECOVER_FIELDS: &[InputField<Input>] = &[REQUIRED_ID, CAP, NO_WAIT];
const ENSURE_FIELDS: &[InputField<Input>] = &[ID, CAP, DAYS, NO_WAIT];

fn ensure_defaults() -> Input {
    Input {
        max_days_left: 7,
        ..Input::default()
    }
}

fn check_consent(input: &Input, _: &clap::ArgMatches) -> Result<(), InputError> {
    input.max_price.map(|_| ()).ok_or(InputError(
        "explicit --max-price or JSON MaxPrice is required, including zero",
    ))
}

fn warning(input: &Input) -> Option<&'static str> {
    input.max_price.is_some_and(|price| price > 0.0 && price.is_finite()).then_some(
        "Warning: pay: true may debit the balance; MaxPrice is a local guard, not a server cap.",
    )
}

const NOTES: &str = "Requires explicit --max-price or JSON MaxPrice, including zero. The account catalog and price quote must agree on a free one-month term. Billing may debit the balance. The SDK rechecks the terms before its single order and confirms by reading. --no-wait retains the attempt for reconciliation. --query is unavailable. Serialize writes across processes per account and kind. Retain a durable pending marker and reconcile with read-only quota reads and console billing after uncertainty.";

macro_rules! operation {
    ($constant:ident, $name:literal, $go:literal, $action:ident, $method:ident, $input:ident, $fields:ident, [$($flag:ident),+], $defaults:path, $help:literal, $notes:literal, $example:literal) => {
        pub(super) const $constant: Operation = {
            const SHAPE: outcome::Shape = outcome::Shape {
                operation: concat!("monitor.", $go),
                action: outcome::Action::$action,
            };
            const INPUT: InputDescriptor<Input> = InputDescriptor {
                defaults: $defaults,
                allow_query: false,
                guard: Some(check_consent),
                ..descriptor($fields)
            };
            Operation {
                name: $name,
                sdk_operation: SHAPE.operation,
                short_help: $help,
                example: Some(concat!("greennode monitor ", $example)),
                kind: Kind::Write,
                notes: &[$notes, NOTES],
                flags: &[$(flag(&$flag)),+],
                presence_required_fields: &[],
                inherited_bindings: &[],
                json_fields: &[],
                extra_flags: &[],
                handler: Some(Handler::new(&INPUT, |operation, matches, invocation| {
                    Box::pin(dispatch::management_with_write_policy(
                        invocation, operation, matches, &INPUT,
                        |ctx, config, input| Box::pin(async move {
                            let result = sdk::Client::new(config).$method(ctx, $input(input)).await;
                            SHAPE.project(result, |out| &out.outcome)
                        }),
                        (OutputView::default(), None, WritePolicy {
                            warning: Some(warning),
                            preflight: Some(|error| SHAPE.preflight(error)),
                            error_writer: Some(|stderr, error, _| SHAPE.write_error(stderr, error.code, error.status)),
                            finish: Some(|stdout, stderr, format, result| SHAPE.finish(stdout, stderr, format, result)),
                        }),
                    ))
                }).with_parse_preflight(ParsePreflight {
                    project: |error| SHAPE.preflight(error),
                    error_writer: Some(|stderr, error, _| SHAPE.write_error(stderr, error.code, error.status)),
                })),
            }
        };
    };
}

operation!(
    CREATE,
    "create-synthetic-quota",
    "CreateSyntheticQuota",
    Create,
    create_synthetic_quota,
    create,
    CREATE_FIELDS,
    [CLASS_ID, PACKAGE_ID, QUANTITY, CAP, NO_WAIT],
    Input::default,
    "Order a free synthetic quota",
    "Requires an absent quota. Confirmation matches package and detail and requires a start at or after SentAt.",
    "create-synthetic-quota --class-id <class-id> --package-id <package-id> --quantity 10 --max-price 0"
);
operation!(
    RENEW,
    "renew-synthetic-quota",
    "RenewSyntheticQuota",
    Renew,
    renew_synthetic_quota,
    renew,
    RENEW_FIELDS,
    [EXPECTED_END_TIME, CAP, DAYS, NO_WAIT],
    Input::default,
    "Renew an active free synthetic quota",
    "ExpectedEndTime must match the active quota. POC and auto-renew must be off. A positive MaxDaysLeft skips early renewal.",
    "renew-synthetic-quota --expected-end-time <expiry> --max-days-left 7 --max-price 0"
);
operation!(
    RECOVER,
    "recover-synthetic-quota",
    "RecoverSyntheticQuota",
    Recover,
    recover_synthetic_quota,
    recover,
    RECOVER_FIELDS,
    [REQUIRED_ID, CAP, NO_WAIT],
    Input::default,
    "Recover an expired free synthetic quota",
    "Requires the expected quota ID and expired IN-TRASH status. POC and auto-renew must be off. Confirmation requires the same ID and package with a later expiry.",
    "recover-synthetic-quota --expected-quota-id <quota-id> --max-price 0"
);
operation!(
    ENSURE,
    "ensure-synthetic-quota",
    "EnsureSyntheticQuota",
    Ensure,
    ensure_synthetic_quota,
    ensure,
    ENSURE_FIELDS,
    [ID, CAP, DAYS, NO_WAIT],
    ensure_defaults,
    "Renew or recover a free synthetic quota",
    "Selects renewal for ACTIVE future expiry or recovery for expired IN-TRASH. POC and auto-renew must be off. MaxDaysLeft defaults to seven in flags and JSON. Explicit zero permits early renewal.",
    "ensure-synthetic-quota --max-days-left 7 --max-price 0"
);

fn create(input: &Input) -> sdk::CreateSyntheticQuotaInput {
    let mut value =
        sdk::CreateSyntheticQuotaInput::new(&input.class_id, &input.package_id, input.quantity);
    value.max_price = input.max_price.unwrap_or_default();
    value.no_wait = input.no_wait;
    value
}
fn renew(input: &Input) -> sdk::RenewSyntheticQuotaInput {
    let mut value = sdk::RenewSyntheticQuotaInput::new(&input.expected_end_time);
    value.max_price = input.max_price.unwrap_or_default();
    value.max_days_left = input.max_days_left;
    value.no_wait = input.no_wait;
    value
}
fn recover(input: &Input) -> sdk::RecoverSyntheticQuotaInput {
    let mut value = sdk::RecoverSyntheticQuotaInput::new(&input.expected_quota_id);
    value.max_price = input.max_price.unwrap_or_default();
    value.no_wait = input.no_wait;
    value
}
fn ensure(input: &Input) -> sdk::EnsureSyntheticQuotaInput {
    sdk::EnsureSyntheticQuotaInput {
        expected_quota_id: input.expected_quota_id.clone(),
        max_price: input.max_price.unwrap_or_default(),
        max_days_left: input.max_days_left,
        no_wait: input.no_wait,
    }
}
