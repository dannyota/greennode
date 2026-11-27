mod quota;

use std::io::Write;

use greennode_sdk::monitor::{
    Client, CreateNotificationQuotaInput, CreateSyntheticQuotaInput, ErrorDetail,
    GetCheckAlarmInput, GetDashboardInput, GetNotificationQuotaInput,
    GetNotificationQuotaUsageInput, GetQuotaStatusInput, ListCheckResponseTimesInput,
    ListDashboardsInput, ListSyntheticPackagesInput, RenewSyntheticQuotaInput,
};

use crate::cli::dispatch::{self, Format, OutputView};
use crate::cli::errors::{Rule, RuleExit, RuleName, RuleOperation};
use crate::cli::input::{InputDescriptor, InputError, InputField, JsonChecks, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, Service};
use crate::cli::render::{self, Value};

#[derive(Default)]
struct Input {
    dashboard_id: String,
    kind: String,
    check_id: String,
    class_id: String,
    package_id: String,
    quantity: i64,
    expected_end_time: String,
    start: String,
    end: String,
    page: i64,
    size: i64,
    system: bool,
    location_id: String,
    alarm_id: String,
    channel_id: String,
    log_project_id: String,
    name: String,
    status: String,
    severity: String,
    channel_type: String,
    search: String,
    billing_status: String,
    description: String,
    class: String,
    retention_days: i64,
    gb_per_day: i64,
}

macro_rules! field {
    ($flag:literal, $json:literal, $field:ident, $go:literal, $rust:ty, $required:expr, $requirement:expr) => {
        InputField {
            json_name: $json,
            flag: Some(Field {
                name: $flag,
                go_type: $go,
                required: $required,
            }),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$field = serde_json::from_value(value).map_err(|_| ())?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$field = matches.get_one::<$rust>($flag).ok_or(())?.to_owned();
                Ok(())
            }),
            requirement: $requirement,
        }
    };
    ($flag:literal, $json:literal, $field:ident, required) => {
        field!(
            $flag,
            $json,
            $field,
            "string",
            String,
            true,
            Requirement::Nonzero(|input| !input.$field.is_empty())
        )
    };
    ($flag:literal, $json:literal, $field:ident, $go:literal, $rust:ty) => {
        field!(
            $flag,
            $json,
            $field,
            $go,
            $rust,
            false,
            Requirement::Optional
        )
    };
}

const DASHBOARD_ID: InputField<Input> =
    field!("dashboard-id", "DashboardID", dashboard_id, required);
const CHECK_ID: InputField<Input> = field!("check-id", "CheckID", check_id, required);
const NOTIFICATION_KIND: InputField<Input> = field!("kind", "Kind", kind, required);
const NOTIFICATION_QUOTA: InputDescriptor<Input> = descriptor(&[NOTIFICATION_KIND]);
const KIND: InputField<Input> = field!("kind", "Kind", kind, "string", String);
const CLASS_ID: InputField<Input> = field!("class-id", "ClassID", class_id, required);
const PACKAGE_ID: InputField<Input> = field!("package-id", "PackageID", package_id, required);
const QUANTITY: InputField<Input> = field!(
    "quantity",
    "Quantity",
    quantity,
    "int",
    i64,
    true,
    Requirement::Nonzero(|input| input.quantity != 0)
);
const EXPECTED_END_TIME: InputField<Input> = field!(
    "expected-end-time",
    "ExpectedEndTime",
    expected_end_time,
    required
);

const fn ignored(name: &'static str) -> InputField<Input> {
    InputField {
        json_name: name,
        flag: None,
        secret: false,
        nullable: false,
        set_json: |_, _| Ok(()),
        set_flag: None,
        requirement: Requirement::Optional,
    }
}

const OPTIONAL_PACKAGE: InputField<Input> =
    field!("package-id", "PackageID", package_id, "string", String);
const NOTIFICATION_QUOTE: InputDescriptor<Input> = descriptor(&[
    NOTIFICATION_KIND,
    OPTIONAL_PACKAGE,
    ignored(MAX_PRICE.name),
    ignored(NO_WAIT.name),
]);

const MAX_PRICE: Field = Field {
    name: "MaxPrice",
    go_type: "float64",
    required: false,
};
const MAX_DAYS_LEFT: Field = Field {
    name: "MaxDaysLeft",
    go_type: "int",
    required: false,
};
const NO_WAIT: Field = Field {
    name: "NoWait",
    go_type: "bool",
    required: false,
};
const CREATE_QUOTE: InputDescriptor<Input> = descriptor(&[
    CLASS_ID,
    PACKAGE_ID,
    QUANTITY,
    ignored(MAX_PRICE.name),
    ignored(NO_WAIT.name),
]);
const RENEW_QUOTE: InputDescriptor<Input> = descriptor(&[
    EXPECTED_END_TIME,
    ignored(MAX_PRICE.name),
    ignored(MAX_DAYS_LEFT.name),
    ignored(NO_WAIT.name),
]);
const QUOTE_INPUT_CHECKS: JsonChecks = JsonChecks {
    validate: check_quote_json,
    read_error: |error| error,
};

fn check_quote_json(bytes: &[u8]) -> Result<(), InputError> {
    #[derive(serde::Deserialize)]
    struct Fields {
        #[serde(rename = "Name")]
        _name: Option<String>,
        #[serde(rename = "Description")]
        _description: Option<String>,
        #[serde(rename = "Class")]
        _class: Option<String>,
        #[serde(rename = "RetentionDays")]
        _retention_days: Option<i64>,
        #[serde(rename = "GBPerDay")]
        _gb_per_day: Option<i64>,

        #[serde(rename = "Kind")]
        _kind: Option<String>,
        #[serde(rename = "ClassID")]
        _class_id: Option<String>,
        #[serde(rename = "PackageID")]
        _package_id: Option<String>,
        #[serde(rename = "Quantity")]
        _quantity: Option<i64>,
        #[serde(rename = "ExpectedEndTime")]
        _expected_end_time: Option<String>,
        #[serde(rename = "MaxPrice")]
        _max_price: Option<f64>,
        #[serde(rename = "MaxDaysLeft")]
        _max_days_left: Option<i64>,
        #[serde(rename = "NoWait")]
        _no_wait: Option<bool>,
    }
    serde_json::from_slice::<Option<Fields>>(bytes)
        .map(|_| ())
        .map_err(|_| InputError("--cli-input-json: invalid quote field"))
}

const START: InputField<Input> = field!("start", "Start", start, "string", String);
const END: InputField<Input> = field!("end", "End", end, "string", String);
const PAGE: InputField<Input> = field!("page", "Page", page, "int", i64);
const SIZE: InputField<Input> = field!("size", "Size", size, "int", i64);
const SYSTEM: InputField<Input> = field!("system", "System", system, "bool", bool);

const fn flag(field: &InputField<Input>) -> Field {
    match field.flag {
        Some(flag) => flag,
        None => panic!("expected a flag"),
    }
}

const fn descriptor(fields: &'static [InputField<Input>]) -> InputDescriptor<Input> {
    InputDescriptor {
        defaults: Input::default,
        fields,
        allow_json: true,
        allow_query: true,
        project_binding: None,
        pre_guard: Some(|_, json| {
            if json == Some("file://") {
                Err(InputError("--cli-input-json: cannot open file"))
            } else {
                Ok(())
            }
        }),
        guard: None,
    }
}

const LIST_SYNTHETIC_PACKAGES: InputDescriptor<Input> = descriptor(&[]);
const QUOTA_STATUS: InputDescriptor<Input> = descriptor(&[KIND]);
const GET_CHECK_ALARM: InputDescriptor<Input> = descriptor(&[CHECK_ID]);
const GET_DASHBOARD: InputDescriptor<Input> = descriptor(&[DASHBOARD_ID]);
const LIST_DASHBOARDS: InputDescriptor<Input> = descriptor(&[PAGE, SIZE, SYSTEM]);
const LIST_RESPONSE_TIMES: InputDescriptor<Input> = descriptor(&[CHECK_ID, START, END]);

macro_rules! read {
    ($name:literal, $sdk:literal, $help:literal, $method:ident, $input:ident, [$($field:ident),*], $value:expr, $view:expr, $notes:expr, $example:literal $(, [$($json:ident),*], $checks:expr)? $(; $project:expr)?) => {
        Operation {
            name: $name,
            sdk_operation: concat!("monitor.", $sdk),
            short_help: $help,
            example: Some(concat!("greennode monitor ", $example)),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[$(flag(&$field)),*],
            json_fields: &[$($($json),*)?],
            extra_flags: &[],
            notes: $notes,
            handler: Some(Handler::new(&$input, |operation, matches, invocation| {
                Box::pin(dispatch::management_custom(invocation, operation, matches, &$input, |ctx, config, input| {
                    Box::pin(async move {
                        let value = ($value)(input);
                        dispatch::project_result(Client::new(config).$method(ctx, value).await, read!(@project $($project)?))
                    })
                }, $view, None $(.or(Some($checks)))?))
            })),
        }
    };
    (@project $project:expr) => { $project };
    (@project) => { std::convert::identity };
}

mod remaining;

fn dashboard_view() -> OutputView {
    OutputView {
        custom: Some(render_dashboards),
        ..OutputView::default()
    }
}

fn render_dashboards(
    mut stdout: &mut dyn Write,
    _: &mut dyn Write,
    output: &render::Projected,
    format: Format,
    queried: bool,
) -> Result<bool, render::Error> {
    if format == Format::Json || queried {
        return Ok(false);
    }
    let Value::Object(mut output) = output.for_query()? else {
        return Ok(false);
    };
    let mut rows = match (output.remove("Items"), output.remove("Dashboard")) {
        (Some(Value::Array(items)), _) => items,
        (_, Some(dashboard)) => vec![dashboard],
        _ => return Ok(false),
    };
    for row in &mut rows {
        if let Value::Object(fields) = row {
            fields.remove("CreatedAt");
        }
    }
    let rows = Value::Array(rows);
    match format {
        Format::Table => render::write_table(&mut stdout, &rows)?,
        Format::Text => render::write_text(&mut stdout, &rows)?,
        Format::Json => return Ok(false),
    }
    Ok(true)
}

pub(super) const SERVICE: Service = Service {
    name: "monitor",
    error_rules: &[
        Rule {
            identity: "monitor-invalid-quota-kind",
            matches: |error| error.detail::<ErrorDetail>() == Some(&ErrorDetail::InvalidQuotaKind),
            name: None,
            exit: Some(RuleExit::Usage),
            message: Some(
                "monitor.GetQuotaStatus: Kind must be synthetic, email, or sms: vngcloud: invalid input",
            ),
        },
        Rule {
            identity: "monitor-quota-changed",
            matches: |error| error.detail::<ErrorDetail>() == Some(&ErrorDetail::QuotaChanged),
            name: Some(RuleName {
                rank: 2,
                code: "QuotaChanged",
                operation: RuleOperation::Omit,
                include_api_status: false,
            }),
            exit: Some(RuleExit::Failure),
            message: Some("quota guard changed"),
        },
        Rule {
            identity: "monitor-unexpected-quota-status",
            matches: |error| error.detail::<ErrorDetail>() == Some(&ErrorDetail::UnexpectedStatus),
            name: Some(RuleName {
                rank: 3,
                code: "UnexpectedStatus",
                operation: RuleOperation::Omit,
                include_api_status: false,
            }),
            exit: Some(RuleExit::Failure),
            message: Some(
                "monitor.QuoteRenewSyntheticQuota: unknown quota status: monitor: unexpected check status",
            ),
        },
        Rule {
            identity: "monitor-quota-status-incomplete",
            matches: |error| {
                error.detail::<ErrorDetail>() == Some(&ErrorDetail::QuotaStatusIncomplete)
            },
            name: Some(RuleName {
                rank: 1,
                code: "QuotaStatusIncomplete",
                operation: RuleOperation::Fixed("monitor.GetQuotaStatus"),
                include_api_status: false,
            }),
            exit: Some(RuleExit::Failure),
            message: Some("monitor: quota status incomplete"),
        },
    ],
    operations: &[
        remaining::LIST_CHECK_ALARM_HISTORY,
        remaining::GET_ALARM,
        remaining::LIST_ALARMS,
        remaining::GET_CHANNEL,
        remaining::LIST_CHANNELS,
        remaining::LIST_CHANNEL_TYPES,
        remaining::GET_CHECK,
        remaining::LIST_CHECKS,
        remaining::LIST_LOCATIONS,
        remaining::GET_LOG_PROJECT,
        remaining::LIST_LOG_PROJECTS,
        remaining::LIST_LOG_PROJECT_CLASSES,
        remaining::QUOTE_CREATE_LOG_PROJECT,
        remaining::GET_QUOTA_USAGE,
        remaining::GET_SYNTHETIC_QUOTA,
        remaining::GET_SYNTHETIC_QUOTA_USAGE,
        remaining::LIST_NOTIFICATION_PACKAGES,
        read!(
            "get-check-alarm",
            "GetCheckAlarm",
            "Read check alarm and location states",
            get_check_alarm,
            GET_CHECK_ALARM,
            [CHECK_ID],
            |input: &Input| GetCheckAlarmInput::new(&input.check_id),
            OutputView::default(),
            &[
                "Reads the current alarm and its location states through the check's HistoryAlarmID. An empty alarm reference is NotFound (exit 4). An unknown definition can return server error 500 (exit 1). A check's Status reports whether it runs. MonitorStatus reports its alarm state."
            ],
            "get-check-alarm --check-id <check-id> --query Alarm"
        ),
        read!(
            "get-dashboard",
            "GetDashboard",
            "Read dashboard metadata",
            get_dashboard,
            GET_DASHBOARD,
            [DASHBOARD_ID],
            |input: &Input| GetDashboardInput::new(&input.dashboard_id),
            dashboard_view(),
            &["Reads one dashboard without widgets. There is no delete command."],
            "get-dashboard --dashboard-id <dashboard-id> --query Dashboard"
        ),
        read!(
            "get-notification-quota",
            "GetNotificationQuota",
            "Read Email or SMS quota",
            get_notification_quota,
            NOTIFICATION_QUOTA,
            [NOTIFICATION_KIND],
            |input: &Input| GetNotificationQuotaInput::new(&input.kind),
            quota::notification_view(),
            &[
                "--kind accepts Email or SMS, case insensitively. Reads quota identity, status, expiry, and AutoRenew. Table and text timestamps show UTC. A missing quota is NotFound (exit 4)."
            ],
            "get-notification-quota --kind Email --query Quota"
        ),
        read!(
            "get-notification-quota-usage",
            "GetNotificationQuotaUsage",
            "Read Email quota usage",
            get_notification_quota_usage,
            NOTIFICATION_QUOTA,
            [NOTIFICATION_KIND],
            |input: &Input| GetNotificationQuotaUsageInput::new(&input.kind),
            quota::notification_usage_view(),
            &[
                "--kind accepts Email, case insensitively. Reads message consumption and descriptions. A missing quota is NotFound (exit 4). SMS usage is refused before requests."
            ],
            "get-notification-quota-usage --kind Email --query Usage"
        ),
        read!(
            "list-check-response-times",
            "ListCheckResponseTimes",
            "List HTTP check response times",
            list_check_response_times,
            LIST_RESPONSE_TIMES,
            [CHECK_ID, START, END],
            |input: &Input| {
                let mut value = ListCheckResponseTimesInput::new(&input.check_id);
                value.start.clone_from(&input.start);
                value.end.clone_from(&input.end);
                value
            },
            OutputView::default(),
            &[
                "HTTP checks only. --start and --end are RFC 3339. --end defaults to now and --start to 24 hours before --end. The window is at most 24 hours. Values are average response times in milliseconds per probe location, in 5-minute buckets for a 24-hour window. Use get-check-alarm for location names."
            ],
            "list-check-response-times --check-id <check-id>"
        ),
        read!(
            "list-dashboards",
            "ListDashboards",
            "List dashboards",
            list_dashboards,
            LIST_DASHBOARDS,
            [PAGE, SIZE, SYSTEM],
            |input: &Input| ListDashboardsInput {
                page: input.page,
                size: input.size,
                system: input.system
            },
            dashboard_view(),
            &[
                "Lists custom dashboards by default. --system lists dashboards GreenNode generates per resource. The read omits widgets. There is no delete command."
            ],
            "list-dashboards"
        ),
        read!(
            "list-synthetic-packages",
            "ListSyntheticPackages",
            "List synthetic quota classes and packages",
            list_synthetic_packages,
            LIST_SYNTHETIC_PACKAGES,
            [],
            |_: &Input| ListSyntheticPackagesInput,
            quota::catalog_view(),
            &[
                "Lists class options, class unit packages, and v1 packages separately, with their own identifiers. Class Packages shows unit API, location, tier, and monthly unit price. Options join only by exact package ID within the class. The v1 catalog cannot establish class option prices. Disabled classes stay visible. AutoRenewEligible is unknown without provider evidence. Settings periods are shown separately. Known free periods are capped by the free maximum. Read-only settings allow this read."
            ],
            "list-synthetic-packages"
        ),
        read!(
            "quota-status",
            "GetQuotaStatus",
            "Read the sequential quota status snapshot",
            get_quota_status,
            QUOTA_STATUS,
            [KIND],
            |input: &Input| GetQuotaStatusInput {
                kind: input.kind.clone()
            },
            quota::status_view(),
            &[
                "--kind optionally selects synthetic, email, or sms, case insensitively. Omitted means all three. Returns a sequential snapshot with one UTC AsOf. Synthetic uses quota, detail, class catalog, and settings reads, with detail price and class and an unknown package name. Notification price is the current list price. Missing quotas produce none rows and exit 0. Failed rows print partial output, ignore --query, and exit 1 as QuotaStatusIncomplete. Null means unknown. False and zero remain distinct. RenewWindowOpen describes the catalog-eligible free Synthetic timing window. NextSafeCommand suggests renewal inside the free window or recovery for expired free trash, only with POC and auto-renew off. Suggested writes require explicit guards from a fresh read. Table and text show time left in days, hours, and minutes. Rows without errors have a blank Error cell. Read-only settings allow this read."
            ],
            "quota-status --kind synthetic"
        ),
        read!(
            "quote-create-notification-quota",
            "QuoteCreateNotificationQuota",
            "Quote one month of Email or SMS quota",
            quote_create_notification_quota,
            NOTIFICATION_QUOTE,
            [NOTIFICATION_KIND, OPTIONAL_PACKAGE],
            |input: &Input| {
                let mut value = CreateNotificationQuotaInput::new(&input.kind);
                value.package_id.clone_from(&input.package_id);
                value
            },
            OutputView::default(),
            &[
                "Estimates one month of Email or SMS capacity without ordering. Omitting --package-id selects the unique zero-price package. An existing quota is refused. MaxPrice and NoWait are ignored. Prices are in VND. Email pre-VAT treatment is inferred and SMS VAT treatment is unverified. GuardPrice is the higher of list and quote prices."
            ],
            "quote-create-notification-quota --kind Email",
            [MAX_PRICE, NO_WAIT],
            QUOTE_INPUT_CHECKS
        ),
        read!(
            "quote-create-synthetic-quota",
            "QuoteCreateSyntheticQuota",
            "Quote one month of new synthetic quota",
            quote_create_synthetic_quota,
            CREATE_QUOTE,
            [CLASS_ID, PACKAGE_ID, QUANTITY],
            |input: &Input| CreateSyntheticQuotaInput::new(
                &input.class_id,
                &input.package_id,
                input.quantity
            ),
            OutputView::default(),
            &[
                "Estimates one month of a selected free package after verifying quota absence and eligibility in the class catalog. Uses class unit prices. Paid offers are refused. MaxPrice and NoWait are ignored. Read-only permits this quote."
            ],
            "quote-create-synthetic-quota --class-id <class-id> --package-id <package-id> --quantity <quantity>",
            [MAX_PRICE, NO_WAIT],
            QUOTE_INPUT_CHECKS
        ),
        read!(
            "quote-renew-synthetic-quota",
            "QuoteRenewSyntheticQuota",
            "Quote one month of synthetic quota renewal",
            quote_renew_synthetic_quota,
            RENEW_QUOTE,
            [EXPECTED_END_TIME],
            |input: &Input| RenewSyntheticQuotaInput::new(&input.expected_end_time),
            OutputView::default(),
            &[
                "Estimates one month from the active quota and its identity-checked detail. ExpectedEndTime must be RFC 3339 and match the quota expiry. MaxPrice, MaxDaysLeft, and NoWait are ignored. Read-only permits this quote."
            ],
            "quote-renew-synthetic-quota --expected-end-time <expected-end-time>",
            [MAX_PRICE, MAX_DAYS_LEFT, NO_WAIT],
            QUOTE_INPUT_CHECKS
        ),
    ],
};
