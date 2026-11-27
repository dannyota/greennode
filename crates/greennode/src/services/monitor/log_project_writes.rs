use super::*;
use greennode_sdk::monitor as sdk;

const NAME: InputField<Input> = field!("name", "Name", name, required);
const DESCRIPTION: InputField<Input> =
    field!("description", "Description", description, "string", String);
const CLASS: InputField<Input> = field!("class", "Class", class, "string", String);
const RETENTION_DAYS: InputField<Input> = field!(
    "retention-days",
    "RetentionDays",
    retention_days,
    "int",
    i64
);
const GB_PER_DAY: InputField<Input> = field!("gb-per-day", "GBPerDay", gb_per_day, "int", i64);
const LOG_PROJECT_ID: InputField<Input> =
    field!("log-project-id", "LogProjectID", log_project_id, required);
const PURGE: InputField<Input> = field!("purge", "Purge", purge, "bool", bool);
const NO_WAIT: InputField<Input> = field!("no-wait", "NoWait", no_wait, "bool", bool);
const CREATE_INPUT: InputDescriptor<Input> = descriptor(&[
    NAME,
    DESCRIPTION,
    CLASS,
    RETENTION_DAYS,
    GB_PER_DAY,
    CAP,
    NO_WAIT,
]);
const DELETE_INPUT: InputDescriptor<Input> = descriptor(&[LOG_PROJECT_ID, PURGE, NO_WAIT]);

fn check_cap(bytes: &[u8]) -> Result<(), InputError> {
    #[derive(serde::Deserialize)]
    struct NumericFields {
        #[serde(rename = "MaxPrice")]
        _max_price: Option<f64>,
        #[serde(rename = "RetentionDays")]
        _retention_days: Option<i64>,
        #[serde(rename = "GBPerDay")]
        _gb_per_day: Option<i64>,
    }
    serde_json::from_slice::<Option<NumericFields>>(bytes)
        .map(|_| ())
        .map_err(|_| InputError("--cli-input-json: invalid log project input"))
}
const CHECKS: JsonChecks = JsonChecks {
    validate: check_cap,
    read_error: |error| error,
};

pub(super) const CREATE: Operation = Operation {
    name: "create-log-project",
    sdk_operation: "monitor.CreateLogProject",
    short_help: "Order a log project",
    example: Some("greennode monitor create-log-project --name <name>"),
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Write,
    flags: &[
        flag(&NAME),
        flag(&DESCRIPTION),
        flag(&CLASS),
        flag(&RETENTION_DAYS),
        flag(&GB_PER_DAY),
        flag(&CAP),
        flag(&NO_WAIT),
    ],
    json_fields: &[],
    extra_flags: &[],
    notes: &[
        "The order sends once and uses the same request body the SDK priced. The default --max-price 0 permits only a quote of zero VND. A negative or nonfinite cap is refused before requests. The exact name search must finish all pages with consistent counts and identities. Without --no-wait, waits up to 120 seconds for the project to become ACTIVE. If an order may have reached the server, list log projects by name before ordering again.",
    ],
    handler: Some(Handler::new(
        &CREATE_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management_custom(
                invocation,
                operation,
                matches,
                &CREATE_INPUT,
                |ctx, config, input| {
                    Box::pin(async move {
                        let mut value = sdk::CreateLogProjectInput::new(&input.name);
                        value.description.clone_from(&input.description);
                        value.class.clone_from(&input.class);
                        value.retention_days = input.retention_days;
                        value.gb_per_day = input.gb_per_day;
                        value.max_price = input.max_price.unwrap_or_default();
                        value.no_wait = input.no_wait;
                        dispatch::project_result(
                            sdk::Client::new(config)
                                .create_log_project(ctx, value)
                                .await,
                            std::convert::identity,
                        )
                    })
                },
                OutputView::default(),
                Some(CHECKS),
            ))
        },
    )),
};

pub(super) const DELETE: Operation = Operation {
    name: "delete-log-project",
    sdk_operation: "monitor.DeleteLogProject",
    short_help: "Trash or purge a log project",
    example: Some("greennode monitor delete-log-project --log-project-id <log-project-id> --yes"),
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::DestructiveWrite,
    flags: &[flag(&LOG_PROJECT_ID), flag(&PURGE), flag(&NO_WAIT)],
    json_fields: &[],
    extra_flags: &[],
    notes: &[
        "Requires --yes. --purge also removes the project from trash. By default, reads the initial project and waits up to 60 seconds for a 404 or a change in its status or billing status. --no-wait skips both reads.",
    ],
    handler: Some(Handler::new(
        &DELETE_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management(
                invocation,
                operation,
                matches,
                &DELETE_INPUT,
                |ctx, config, input| {
                    Box::pin(async move {
                        let mut value = sdk::DeleteLogProjectInput::new(&input.log_project_id);
                        value.purge = input.purge;
                        value.no_wait = input.no_wait;
                        dispatch::project_result(
                            sdk::Client::new(config)
                                .delete_log_project(ctx, value)
                                .await,
                            std::convert::identity,
                        )
                    })
                },
            ))
        },
    )),
};
