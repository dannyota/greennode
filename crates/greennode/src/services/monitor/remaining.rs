use super::*;
use greennode_sdk::monitor as sdk;

const LOCATION_ID: InputField<Input> =
    field!("location-id", "LocationID", location_id, "string", String);
const ALARM_ID: InputField<Input> = field!("alarm-id", "AlarmID", alarm_id, required);
const CHANNEL_ID: InputField<Input> = field!("channel-id", "ChannelID", channel_id, required);
const LOG_PROJECT_ID: InputField<Input> =
    field!("log-project-id", "LogProjectID", log_project_id, required);
const NAME: InputField<Input> = field!("name", "Name", name, "string", String);
const STATUS: InputField<Input> = field!("status", "Status", status, "string", String);
const SEVERITY: InputField<Input> = field!("severity", "Severity", severity, "string", String);
const CHANNEL_TYPE: InputField<Input> = field!("type", "Type", channel_type, "string", String);
const SEARCH: InputField<Input> = field!("search", "Query", search, "string", String);
const BILLING_STATUS: InputField<Input> = field!(
    "billing-status",
    "BillingStatus",
    billing_status,
    "string",
    String
);
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
const REQUIRED_NAME: InputField<Input> = field!("name", "Name", name, required);
const EMPTY: InputDescriptor<Input> = descriptor(&[]);
const LIST_CHECK_ALARM_HISTORY_INPUT: InputDescriptor<Input> =
    descriptor(&[CHECK_ID, START, END, LOCATION_ID]);
pub(super) const LIST_CHECK_ALARM_HISTORY: Operation = read!(
    "list-check-alarm-history",
    "ListCheckAlarmHistory",
    "List check alarm history",
    list_check_alarm_history,
    LIST_CHECK_ALARM_HISTORY_INPUT,
    [CHECK_ID, START, END, LOCATION_ID],
    |input: &Input| history(input),
    OutputView::default(),
    &[
        "--start and --end are RFC 3339 strings. End defaults to now and Start to 24 hours before End. Invalid, empty, or reversed ranges at millisecond precision exit 2 before requests. --location-id selects one probe location. Items contains only returned transitions: paging and truncation are unknown, and no state at the window start is supplied. Empty Items proves neither that no transition occurred nor that an alarm exists. Use get-check-alarm for current state."
    ],
    "list-check-alarm-history --check-id <check-id>"
);
const GET_ALARM_INPUT: InputDescriptor<Input> = descriptor(&[ALARM_ID]);
pub(super) const GET_ALARM: Operation = read!(
    "get-alarm",
    "GetAlarm",
    "Get alarm",
    get_alarm,
    GET_ALARM_INPUT,
    [ALARM_ID],
    |input: &Input| sdk::GetAlarmInput::new(&input.alarm_id),
    OutputView::default(),
    &[
        "A missing alarm exits 1, not 4: the API answers an unknown ID with a 500, not a 404, so this command never reports the NotFound error class."
    ],
    "get-alarm --alarm-id <alarm-id> --query Alarm"
);
const LIST_ALARMS_INPUT: InputDescriptor<Input> =
    descriptor(&[NOTIFICATION_KIND, NAME, STATUS, SEVERITY, PAGE, SIZE]);
pub(super) const LIST_ALARMS: Operation = read!(
    "list-alarms",
    "ListAlarms",
    "List alarms",
    list_alarms,
    LIST_ALARMS_INPUT,
    [NOTIFICATION_KIND, NAME, STATUS, SEVERITY, PAGE, SIZE],
    |input: &Input| sdk::ListAlarmsInput {
        kind: input.kind.clone(),
        name: input.name.clone(),
        status: input.status.clone(),
        severity: input.severity.clone(),
        page: input.page,
        size: input.size
    },
    OutputView::default(),
    &[],
    "list-alarms --kind Log"
);
const GET_CHANNEL_INPUT: InputDescriptor<Input> = descriptor(&[CHANNEL_ID]);
pub(super) const GET_CHANNEL: Operation = read!(
    "get-channel", "GetChannel", "Get channel", get_channel, GET_CHANNEL_INPUT, [CHANNEL_ID],
    |input: &Input| sdk::GetChannelInput::new(&input.channel_id), OutputView::default(), &["Redacts every header value and every Address except an Email, SMS, or Telegram channel's, since a Webhook, Slack, or other channel's Address can carry a bearer token; only Email, SMS, and Telegram addresses print in full."], "get-channel --channel-id <channel-id> --query Channel"; |mut output: sdk::GetChannelOutput| { output.channel = output.channel.for_display(); output }
);
const LIST_CHANNELS_INPUT: InputDescriptor<Input> = descriptor(&[CHANNEL_TYPE, PAGE, SIZE]);
pub(super) const LIST_CHANNELS: Operation = read!(
    "list-channels", "ListChannels", "List channels", list_channels, LIST_CHANNELS_INPUT, [CHANNEL_TYPE, PAGE, SIZE],
    |input: &Input| sdk::ListChannelsInput { r#type:input.channel_type.clone(),page:input.page,size:input.size }, OutputView::default(), &["Redacts every header value and every Address except an Email, SMS, or Telegram channel's, since a Webhook, Slack, or other channel's Address can carry a bearer token; only Email, SMS, and Telegram addresses print in full."], "list-channels"; |mut output: sdk::ListChannelsOutput| { output.items = output.items.into_iter().map(sdk::Channel::for_display).collect(); output }
);
pub(super) const LIST_CHANNEL_TYPES: Operation = read!(
    "list-channel-types",
    "ListChannelTypes",
    "List channel types",
    list_channel_types,
    EMPTY,
    [],
    |_: &Input| sdk::ListChannelTypesInput,
    OutputView::default(),
    &[],
    "list-channel-types"
);
const GET_CHECK_INPUT: InputDescriptor<Input> = descriptor(&[CHECK_ID]);
pub(super) const GET_CHECK: Operation = read!(
    "get-check",
    "GetCheck",
    "Get check",
    get_check,
    GET_CHECK_INPUT,
    [CHECK_ID],
    |input: &Input| sdk::GetCheckInput::new(&input.check_id),
    OutputView::default(),
    &[],
    "get-check --check-id <check-id> --query Check"
);
pub(super) const LIST_CHECKS: Operation = read!(
    "list-checks",
    "ListChecks",
    "List checks",
    list_checks,
    EMPTY,
    [],
    |_: &Input| sdk::ListChecksInput,
    OutputView::default(),
    &[],
    "list-checks"
);
pub(super) const LIST_LOCATIONS: Operation = read!(
    "list-locations",
    "ListLocations",
    "List locations",
    list_locations,
    EMPTY,
    [],
    |_: &Input| sdk::ListLocationsInput,
    OutputView::default(),
    &[],
    "list-locations"
);
const GET_LOG_PROJECT_INPUT: InputDescriptor<Input> = descriptor(&[LOG_PROJECT_ID]);
pub(super) const GET_LOG_PROJECT: Operation = read!(
    "get-log-project",
    "GetLogProject",
    "Get log project",
    get_log_project,
    GET_LOG_PROJECT_INPUT,
    [LOG_PROJECT_ID],
    |input: &Input| sdk::GetLogProjectInput::new(&input.log_project_id),
    OutputView::default(),
    &[],
    "get-log-project --log-project-id <log-project-id> --query LogProject"
);
const LIST_LOG_PROJECTS_INPUT: InputDescriptor<Input> =
    descriptor(&[SEARCH, BILLING_STATUS, PAGE, SIZE]);
pub(super) const LIST_LOG_PROJECTS: Operation = read!(
    "list-log-projects",
    "ListLogProjects",
    "List log projects",
    list_log_projects,
    LIST_LOG_PROJECTS_INPUT,
    [SEARCH, BILLING_STATUS, PAGE, SIZE],
    |input: &Input| sdk::ListLogProjectsInput {
        query: input.search.clone(),
        billing_status: input.billing_status.clone(),
        page: input.page,
        size: input.size
    },
    OutputView::default(),
    &[],
    "list-log-projects"
);
pub(super) const LIST_LOG_PROJECT_CLASSES: Operation = read!(
    "list-log-project-classes",
    "ListLogProjectClasses",
    "List log project classes",
    list_log_project_classes,
    EMPTY,
    [],
    |_: &Input| sdk::ListLogProjectClassesInput,
    OutputView::default(),
    &[],
    "list-log-project-classes"
);
const QUOTE_CREATE_LOG_PROJECT_INPUT: InputDescriptor<Input> = descriptor(&[
    REQUIRED_NAME,
    DESCRIPTION,
    CLASS,
    RETENTION_DAYS,
    GB_PER_DAY,
    ignored(MAX_PRICE.name),
    ignored(NO_WAIT.name),
]);
pub(super) const QUOTE_CREATE_LOG_PROJECT: Operation = read!(
    "quote-create-log-project",
    "QuoteCreateLogProject",
    "Quote create log project",
    quote_create_log_project,
    QUOTE_CREATE_LOG_PROJECT_INPUT,
    [
        REQUIRED_NAME,
        DESCRIPTION,
        CLASS,
        RETENTION_DAYS,
        GB_PER_DAY
    ],
    |input: &Input| log_quote(input),
    OutputView::default(),
    &[
        "Ignores MaxPrice and NoWait even when an inline --cli-input-json value sets them: both govern only create-log-project's own price ceiling and wait, never this read, which neither orders anything nor waits. Prices are VND totals that include VAT; the API gives no VAT breakdown."
    ],
    "quote-create-log-project --name <name>",
    [MAX_PRICE, NO_WAIT],
    QUOTE_INPUT_CHECKS
);
pub(super) const GET_QUOTA_USAGE: Operation = read!(
    "get-quota-usage",
    "GetQuotaUsage",
    "Get quota usage",
    get_quota_usage,
    EMPTY,
    [],
    |_: &Input| sdk::GetQuotaUsageInput,
    OutputView::default(),
    &[
        "Reads composite usage and freeEmailQuota/freeSmsQuota. UnmodeledKinds names nonempty usage arrays whose item shapes are unknown; they contribute no invented totals. Alert emails consume Email quota usage, while the synthetic email counter can stay zero. Allowances depend on the account and package. See Monitor Quotas."
    ],
    "get-quota-usage --query Usage"
);
pub(super) const GET_SYNTHETIC_QUOTA: Operation = read!(
    "get-synthetic-quota",
    "GetSyntheticQuota",
    "Get synthetic quota",
    get_synthetic_quota,
    EMPTY,
    [],
    |_: &Input| sdk::GetSyntheticQuotaInput,
    quota::synthetic_view(),
    &[
        "Reads quota identity, status, expiry, AutoRenew, and package detail. Table/text timestamps show UTC and absent prices show unknown; JSON preserves SDK values. ChannelLimits preserves null versus explicit zero; legacy integer limits collapse null to zero. Synthetic quota expiry stops checks. A missing quota is NotFound; check monitor quota-status --kind synthetic and use monitor create-synthetic-quota for a free quota. Auto-renew enable sends once; the observed free Basic quota returned BadRequest (400, exit 1). Disable remains unavailable. Guarded free create, recovery, renewal, and destructive delete are available. See Monitor Synthetic Quota."
    ],
    "get-synthetic-quota --query Quota"
);
pub(super) const GET_SYNTHETIC_QUOTA_USAGE: Operation = read!(
    "get-synthetic-quota-usage",
    "GetSyntheticQuotaUsage",
    "Get synthetic quota usage",
    get_synthetic_quota_usage,
    EMPTY,
    [],
    |_: &Input| sdk::GetSyntheticQuotaUsageInput,
    OutputView::default(),
    &[
        "Reads synthetic usage counters and their descriptions. Alert emails consume the Email quota, rather than the synthetic email counter. See Monitor Quotas."
    ],
    "get-synthetic-quota-usage --query Usage"
);
const LIST_NOTIFICATION_PACKAGES_INPUT: InputDescriptor<Input> = descriptor(&[NOTIFICATION_KIND]);
pub(super) const LIST_NOTIFICATION_PACKAGES: Operation = read!(
    "list-notification-packages",
    "ListNotificationPackages",
    "List notification packages",
    list_notification_packages,
    LIST_NOTIFICATION_PACKAGES_INPUT,
    [NOTIFICATION_KIND],
    |input: &Input| sdk::ListNotificationPackagesInput::new(&input.kind),
    quota::package_view(),
    &[
        "--kind accepts Email or SMS, case insensitively. Prices are in VND; absent prices print unknown in table/text and null in JSON. A zero-price package is not an automatic allowance. Email and SMS quota reads, quotes, and orders are available; SMS usage refuses locally."
    ],
    "list-notification-packages --kind Email"
);

fn history(input: &Input) -> sdk::ListCheckAlarmHistoryInput {
    let mut value = sdk::ListCheckAlarmHistoryInput::new(&input.check_id);
    value.start.clone_from(&input.start);
    value.end.clone_from(&input.end);
    value.location_id.clone_from(&input.location_id);
    value
}

fn log_quote(input: &Input) -> sdk::CreateLogProjectInput {
    let mut value = sdk::CreateLogProjectInput::new(&input.name);
    value.description.clone_from(&input.description);
    value.class.clone_from(&input.class);
    value.retention_days = input.retention_days;
    value.gb_per_day = input.gb_per_day;
    value
}
