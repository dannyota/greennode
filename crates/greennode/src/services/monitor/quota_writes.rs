use super::*;
use crate::cli::dispatch::WritePolicy;
use crate::cli::op::ParsePreflight;
use clap::{ArgMatches, parser::ValueSource};
use greennode_sdk::monitor as sdk;
use serde::Serialize;

const EFFECT: &str = "Deleting this quota permanently deletes every synthetic check.";
const CAP: InputField<Input> = field!("max-price", "MaxPrice", max_price, "float64", f64);
const NO_WAIT: InputField<Input> = field!("no-wait", "NoWait", no_wait, "bool", bool);
const PURGE: InputField<Input> = field!("purge", "Purge", purge, "bool", bool);
const EXPECTED: InputField<Input> = field!(
    "expected-quota-id",
    "ExpectedQuotaID",
    expected_quota_id,
    "string",
    String
);
const REQUIRED_EXPECTED: InputField<Input> = field!(
    "expected-quota-id",
    "ExpectedQuotaID",
    expected_quota_id,
    required
);
const ENABLED_FIELD: Field = Field {
    name: "enabled",
    go_type: "*bool",
    required: true,
};
const ENABLED: InputField<Input> = InputField {
    json_name: "Enabled",
    flag: Some(ENABLED_FIELD),
    secret: false,
    nullable: false,
    requirement: Requirement::Presence,
    set_json: |input, value| {
        input.enabled = Some(serde_json::from_value(value).map_err(|_| ())?);
        Ok(())
    },
    set_flag: Some(|input, matches| {
        input.enabled = matches.get_one::<bool>("enabled").copied();
        Ok(())
    }),
};
const JSON_PURGE: InputField<Input> = InputField {
    flag: None,
    set_flag: None,
    ..PURGE
};
const CREATE_INPUT: InputDescriptor<Input> =
    descriptor(&[NOTIFICATION_KIND, OPTIONAL_PACKAGE, CAP, NO_WAIT]);
const DELETE_INPUT: InputDescriptor<Input> = descriptor(&[NOTIFICATION_KIND, EXPECTED, JSON_PURGE]);
const DELETE_SYNTH_INPUT: InputDescriptor<Input> = InputDescriptor {
    guard: Some(check_delete_query),
    ..descriptor(&[PURGE, REQUIRED_EXPECTED])
};
const NOTIFY_RENEW_INPUT: InputDescriptor<Input> =
    descriptor(&[NOTIFICATION_KIND, ENABLED, CAP, EXPECTED]);
const SYNTH_RENEW_INPUT: InputDescriptor<Input> = descriptor(&[ENABLED, CAP, EXPECTED]);

fn check_cap(bytes: &[u8]) -> Result<(), InputError> {
    #[derive(serde::Deserialize)]
    struct Fields {
        #[serde(rename = "MaxPrice")]
        _max_price: Option<f64>,
    }
    serde_json::from_slice::<Option<Fields>>(bytes)
        .map(|_| ())
        .map_err(|_| InputError("--cli-input-json: invalid quota input"))
}
const CHECKS: JsonChecks = JsonChecks {
    validate: check_cap,
    read_error: |error| error,
};

fn check_delete_query(_: &Input, matches: &ArgMatches) -> Result<(), InputError> {
    if matches.value_source("query") == Some(ValueSource::CommandLine) {
        return Err(InputError(
            "--query is unavailable for synthetic quota deletion effects",
        ));
    }
    Ok(())
}
fn notification_warning(input: &Input) -> Option<&'static str> {
    (input.max_price>0.0 && input.max_price.is_finite()).then_some("Warning: pay: true may debit the balance; MaxPrice is a local guard, not a server cap. Email quotes are inferred pre-VAT; SMS VAT treatment is unverified.")
}
fn synth_warning(input: &Input) -> Option<&'static str> {
    (input.enabled==Some(true)).then_some("Warning: auto-renew on a free quota is refused by the server (400); POC quota behavior is unverified. The error remains visible.")
}

macro_rules! write {
    ($name:literal,$go:literal,$title:literal,$method:ident,$desc:ident,$kind:ident,$flags:expr,$json:expr,$input:expr,$policy:expr,$view:expr,$project:expr,$notes:expr,$example:literal $(; $preflight:expr)?)=>{
        Operation{name:$name,sdk_operation:concat!("monitor.",$go),short_help:$title,example:Some(concat!("greennode monitor ",$example)),
            presence_required_fields:&[],inherited_bindings:&[],kind:Kind::$kind,flags:$flags,json_fields:$json,extra_flags:&[],notes:$notes,
            handler:Some(Handler::new(&$desc,|operation,matches,invocation|Box::pin(dispatch::management_with_write_policy(invocation,operation,matches,&$desc,|ctx,config,input|{
                Box::pin(async move{dispatch::project_result(sdk::Client::new(config).$method(ctx,($input)(input)).await,$project)})
            },($view,Some(CHECKS),$policy))))$(.with_parse_preflight($preflight))?)
        }
    };
}
pub(super) const CREATE: Operation = write!(
    "create-notification-quota",
    "CreateNotificationQuota",
    "Order Email or SMS quota",
    create_notification_quota,
    CREATE_INPUT,
    Write,
    &[
        flag(&NOTIFICATION_KIND),
        flag(&OPTIONAL_PACKAGE),
        flag(&CAP),
        flag(&NO_WAIT)
    ],
    &[],
    create,
    WritePolicy {
        warning: Some(notification_warning),
        ..WritePolicy::default()
    },
    quota::synthetic_view(),
    std::convert::identity,
    &[
        "Orders one month of Email or SMS quota. A positive --max-price is consent to the higher of the package and quote prices. Billing may debit the balance. --no-wait leaves the attempt open for reconciliation. Serialize writes across processes per account and kind. Retain a durable pending marker and reconcile with read-only quota reads and console billing after uncertainty."
    ],
    "create-notification-quota --kind Email"
);
pub(super) const DELETE: Operation = write!(
    "delete-notification-quota",
    "DeleteNotificationQuota",
    "Delete an Email or SMS quota",
    delete_notification_quota,
    DELETE_INPUT,
    DestructiveWrite,
    &[flag(&NOTIFICATION_KIND), flag(&EXPECTED)],
    &[Field {
        name: "Purge",
        go_type: "bool",
        required: false
    }],
    delete,
    WritePolicy::default(),
    quota::synthetic_view(),
    std::convert::identity,
    &[
        "Requires --yes and an active notification quota with auto-renew and POC off. The optional ExpectedQuotaID guards the read snapshot. The provider request is per kind. Purge is unsupported. Serialize writes across processes per account and kind, and reconcile after uncertainty."
    ],
    "delete-notification-quota --kind Email --yes"
);
pub(super) const RENEW_NOTIFY: Operation = write!(
    "set-notification-quota-auto-renew",
    "SetNotificationQuotaAutoRenew",
    "Disable notification quota auto-renew",
    set_notification_quota_auto_renew,
    NOTIFY_RENEW_INPUT,
    Write,
    &[
        flag(&NOTIFICATION_KIND),
        ENABLED_FIELD,
        flag(&CAP),
        flag(&EXPECTED)
    ],
    &[],
    notification_renew,
    WritePolicy::default(),
    quota::synthetic_view(),
    std::convert::identity,
    &[
        "Requires explicit --enabled=false. Enabling is refused before requests. Optional ExpectedQuotaID binds the intended snapshot. A later read confirms the setting. Serialize writes per account and kind."
    ],
    "set-notification-quota-auto-renew --kind Email --enabled=false"
);
pub(super) const RENEW_SYNTH: Operation = write!(
    "set-synthetic-quota-auto-renew",
    "SetSyntheticQuotaAutoRenew",
    "Enable synthetic quota auto-renew",
    set_synthetic_quota_auto_renew,
    SYNTH_RENEW_INPUT,
    Write,
    &[ENABLED_FIELD, flag(&CAP), flag(&EXPECTED)],
    &[],
    synthetic_renew,
    WritePolicy {
        warning: Some(synth_warning),
        ..WritePolicy::default()
    },
    quota::synthetic_view(),
    std::convert::identity,
    &[
        "Requires explicit --enabled=true. Disabling is unsupported. The SDK quotes the free quota, checks the frozen terms, sends once, and confirms by reading. The free quota server refusal remains visible."
    ],
    "set-synthetic-quota-auto-renew --enabled=true"
);
pub(super) const DELETE_SYNTH: Operation = write!("delete-synthetic-quota","DeleteSyntheticQuota","Delete a synthetic quota and its checks",delete_synthetic_quota,DELETE_SYNTH_INPUT,DestructiveWrite,
    &[flag(&PURGE),flag(&REQUIRED_EXPECTED)],&[],delete_synthetic,
    WritePolicy{preflight:Some(delete_preflight),..WritePolicy::default()},OutputView::default(),delete_effect,
    &["Deleting this quota permanently deletes every synthetic check. Requires --yes and --expected-quota-id. Purge and --query are unsupported. Only an exact source 404 confirms deletion. Serialize per account and kind, retain a durable pending marker, and reconcile with reads after uncertainty."],
    "delete-synthetic-quota --expected-quota-id <quota-id> --yes"; ParsePreflight{project:delete_preflight,error_writer:None});

fn create(input: &Input) -> sdk::CreateNotificationQuotaInput {
    let mut value = sdk::CreateNotificationQuotaInput::new(&input.kind);
    value.package_id.clone_from(&input.package_id);
    value.max_price = input.max_price;
    value.no_wait = input.no_wait;
    value
}
fn delete(input: &Input) -> sdk::DeleteNotificationQuotaInput {
    let mut value = sdk::DeleteNotificationQuotaInput::new(&input.kind);
    value.expected_quota_id.clone_from(&input.expected_quota_id);
    value.purge = input.purge;
    value
}
fn notification_renew(input: &Input) -> sdk::SetNotificationQuotaAutoRenewInput {
    let mut value = sdk::SetNotificationQuotaAutoRenewInput::new(
        &input.kind,
        input.enabled.unwrap_or_default(),
    );
    value.expected_quota_id.clone_from(&input.expected_quota_id);
    value.max_price = input.max_price;
    value
}
fn synthetic_renew(input: &Input) -> sdk::SetSyntheticQuotaAutoRenewInput {
    let mut value = sdk::SetSyntheticQuotaAutoRenewInput::new(input.enabled.unwrap_or_default());
    value.expected_quota_id.clone_from(&input.expected_quota_id);
    value.max_price = input.max_price;
    value
}
fn delete_synthetic(input: &Input) -> sdk::DeleteSyntheticQuotaInput {
    let mut value = sdk::DeleteSyntheticQuotaInput::new(&input.expected_quota_id);
    value.purge = input.purge;
    value
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct DeleteEffect {
    quota: Option<QuotaDisplay>,
    stage: String,
    purged: bool,
    effect: &'static str,
}
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct QuotaDisplay {
    #[serde(rename = "ID")]
    id: String,
    name: String,
    #[serde(rename = "Type")]
    quota_type: String,
    status: String,
    #[serde(rename = "PackageID")]
    package_id: String,
    start_time: String,
    end_time: String,
    created_at: String,
    updated_at: String,
    auto_renew: sdk::ResponseField<bool>,
    #[serde(rename = "POC")]
    poc: sdk::ResponseField<bool>,
    system: sdk::ResponseField<bool>,
    auto_renew_period: sdk::ResponseField<i64>,
}
fn quota_time(value: sdk::ResponseField<sdk::QuotaTimestamp>) -> String {
    let sdk::ResponseField::Value(value) = value else {
        return "<redacted>".into();
    };
    let text = value
        .to_utc()
        .and_then(|time| serde_json::to_value(time).ok());
    match text {
        Some(serde_json::Value::String(text)) => {
            text.trim_end_matches('Z').replace('T', " ") + " UTC"
        }
        _ => String::new(),
    }
}
fn delete_effect(value: sdk::DeleteSyntheticQuotaOutput) -> DeleteEffect {
    DeleteEffect {
        quota: value.quota.map(|quota| QuotaDisplay {
            id: quota.id,
            name: quota.name,
            quota_type: quota.quota_type,
            status: quota.status,
            package_id: quota.package_id,
            start_time: quota_time(quota.start_time),
            end_time: quota_time(quota.end_time),
            created_at: quota_time(quota.created_at),
            updated_at: quota_time(quota.updated_at),
            auto_renew: quota.auto_renew,
            poc: quota.poc,
            system: quota.system,
            auto_renew_period: quota.auto_renew_period,
        }),
        stage: value.stage,
        purged: value.purged,
        effect: EFFECT,
    }
}
fn delete_preflight(_: &dispatch::LocalError) -> Option<render::Projected> {
    render::project(&DeleteEffect {
        quota: None,
        stage: String::new(),
        purged: false,
        effect: EFFECT,
    })
    .ok()
}
