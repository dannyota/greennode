mod quota;

use std::io::Write;

use greennode_sdk::monitor::{
    Client, ErrorDetail, GetDashboardInput, GetQuotaStatusInput, ListCheckResponseTimesInput,
    ListDashboardsInput, ListSyntheticPackagesInput,
};

use crate::cli::dispatch::{self, Format, OutputView};
use crate::cli::errors::{Rule, RuleExit, RuleName, RuleOperation};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, Service};
use crate::cli::render::{self, Value};

#[derive(Default)]
struct Input {
    dashboard_id: String,
    kind: String,
    check_id: String,
    start: String,
    end: String,
    page: i64,
    size: i64,
    system: bool,
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
const KIND: InputField<Input> = field!("kind", "Kind", kind, "string", String);
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
const GET_DASHBOARD: InputDescriptor<Input> = descriptor(&[DASHBOARD_ID]);
const LIST_DASHBOARDS: InputDescriptor<Input> = descriptor(&[PAGE, SIZE, SYSTEM]);
const LIST_RESPONSE_TIMES: InputDescriptor<Input> = descriptor(&[CHECK_ID, START, END]);

macro_rules! read {
    ($name:literal, $sdk:literal, $help:literal, $method:ident, $input:ident, [$($field:ident),*], $value:expr, $view:expr, $notes:expr, $example:literal) => {
        Operation {
            name: $name,
            sdk_operation: concat!("monitor.", $sdk),
            short_help: $help,
            example: Some(concat!("greennode monitor ", $example)),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[$(flag(&$field)),*],
            json_fields: &[],
            extra_flags: &[],
            notes: $notes,
            handler: Some(Handler::new(&$input, |operation, matches, invocation| {
                Box::pin(dispatch::management_with_view(invocation, operation, matches, &$input, |ctx, config, input| {
                    Box::pin(async move {
                        let value = ($value)(input);
                        dispatch::project_result(Client::new(config).$method(ctx, value).await, std::convert::identity)
                    })
                }, $view))
            })),
        }
    };
}

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
    ],
};
