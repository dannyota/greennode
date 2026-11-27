use super::*;
use crate::cli::dispatch::WritePolicy;
use crate::cli::op::ParsePreflight;
use clap::{ArgMatches, parser::ValueSource};
use greennode_sdk::monitor as sdk;
use serde::Serialize;

const EFFECT: &str = "Deleting this quota permanently deletes every synthetic check.";
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

fn check_delete_query(_: &Input, matches: &ArgMatches) -> Result<(), InputError> {
    if matches.value_source("query") == Some(ValueSource::CommandLine) {
        return Err(InputError(
            "--query is unavailable for synthetic quota deletion effects",
        ));
    }
    Ok(())
}
fn notification_warning(input: &Input) -> Option<&'static str> {
    input.max_price.is_some_and(|price| price > 0.0 && price.is_finite()).then_some("Warning: pay: true may debit the balance; MaxPrice is a local guard, not a server cap. Email quotes are inferred pre-VAT; SMS VAT treatment is unverified.")
}
fn synth_warning(input: &Input) -> Option<&'static str> {
    (input.enabled==Some(true)).then_some("Warning: auto-renew on a free quota is refused by the server (400); POC quota behavior is unverified. The error remains visible.")
}

macro_rules! write {
    ($name:literal,$go:literal,$title:literal,$method:ident,$desc:ident,$kind:ident,$flags:expr,$json:expr,$input:expr,$policy:expr,$view:expr,$project:expr,$notes:expr,$example:literal $(; $preflight:expr)?)=>{
        Operation{name:$name,sdk_operation:concat!("monitor.",$go),short_help:$title,example:Some(concat!("greennode monitor ",$example)),
            presence_required_fields:&[],inherited_bindings:&[],kind:Kind::$kind,flags:$flags,json_fields:$json,extra_flags:&[],notes:$notes,
            handler:Some(Handler::new(&$desc,|operation,matches,invocation|Box::pin(dispatch::management_with_write_policy(invocation,operation,matches,&$desc,|ctx,config,input|{
                Box::pin(async move{($project)(sdk::Client::new(config).$method(ctx,($input)(input)).await)})
            },($view,None,$policy))))$(.with_parse_preflight($preflight))?)
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
    project_value,
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
    project_value,
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
    project_value,
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
    project_value,
    &[
        "Requires explicit --enabled=true. Disabling is unsupported. The SDK quotes the free quota, checks the frozen terms, sends once, and confirms by reading. The free quota server refusal remains visible."
    ],
    "set-synthetic-quota-auto-renew --enabled=true"
);
pub(super) const DELETE_SYNTH: Operation = write!("delete-synthetic-quota","DeleteSyntheticQuota","Delete a synthetic quota and its checks",delete_synthetic_quota,DELETE_SYNTH_INPUT,DestructiveWrite,
    &[flag(&PURGE),flag(&REQUIRED_EXPECTED)],&[],delete_synthetic,
    WritePolicy{preflight:Some(delete_preflight),finish:Some(finish_delete),..WritePolicy::default()},OutputView::default(),project_delete,
    &["Deleting this quota permanently deletes every synthetic check. Requires --yes and --expected-quota-id. Purge and --query are unsupported. Only an exact source 404 confirms deletion. Serialize per account and kind, retain a durable pending marker, and reconcile with reads after uncertainty."],
    "delete-synthetic-quota --expected-quota-id <quota-id> --yes"; ParsePreflight{project:delete_preflight,error_writer:None});

#[expect(
    clippy::result_large_err,
    reason = "The dispatcher retains the typed error"
)]
fn project_value<T: Serialize>(
    result: Result<T, greennode_sdk::Failure<T>>,
) -> dispatch::ExecutionResult {
    dispatch::project_result(result, std::convert::identity)
}

#[expect(
    clippy::result_large_err,
    reason = "The dispatcher retains the typed error"
)]
fn project_delete(
    result: Result<
        sdk::DeleteSyntheticQuotaOutput,
        greennode_sdk::Failure<sdk::DeleteSyntheticQuotaOutput>,
    >,
) -> dispatch::ExecutionResult {
    let project = |value: sdk::DeleteSyntheticQuotaOutput| {
        let effect = delete_effect(value);
        render::project(&effect).or_else(|_| render::project(&fallback_delete(&effect.stage)))
    };
    match result {
        Ok(value) => {
            let effect = delete_effect(value);
            match render::project(&effect) {
                Ok(output) => Ok(Some(Ok(output))),
                Err(_) => Err(dispatch::ExecutionFailure::Projection(
                    render::project(&fallback_delete(&effect.stage)).ok(),
                )),
            }
        }
        Err(failure) => {
            let (error, partial, dispatch) = failure.into_parts();
            Err(dispatch::OperationFailure {
                error,
                partial: partial.and_then(|value| project(value).ok()),
                dispatch,
                query_partial: false,
            }
            .into())
        }
    }
}

fn fallback_delete(stage: &str) -> DeleteEffect {
    DeleteEffect {
        quota: None,
        stage: if matches!(stage, "sent" | "settled") {
            "sent".into()
        } else {
            stage.into()
        },
        purged: false,
        effect: EFFECT,
    }
}

fn finish_delete(
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    format: Format,
    result: dispatch::ExecutionResult,
) -> u8 {
    use super::synthetic_writes::outcome::encode;
    use crate::cli::errors::{self, Envelope};
    use dispatch::ExecutionFailure;
    use greennode_sdk::WriteDispatch;

    let (output, failure, projection_failed) = match result {
        Ok(Some(Ok(output))) => (Some(output), None, false),
        Err(ExecutionFailure::Projection(output)) => (output, None, true),
        Err(ExecutionFailure::Sdk(failure)) => (
            failure.partial,
            Some((failure.error, failure.dispatch)),
            false,
        ),
        Err(ExecutionFailure::Local(error)) => return dispatch::local(stderr, error),
        Ok(_) => (None, None, true),
    };
    let dispatch_uncertain = failure
        .as_ref()
        .is_some_and(|(_, dispatch)| *dispatch == WriteDispatch::MayHaveBeenSent);
    let stage = output
        .as_ref()
        .and_then(|output| match output.for_query().ok()? {
            Value::Object(mut fields) => match fields.remove("Stage")? {
                Value::String(stage) => Some(stage),
                _ => None,
            },
            _ => None,
        });
    let stage = stage
        .as_deref()
        .unwrap_or(if dispatch_uncertain { "sent" } else { "" });
    let needs_reconciliation = matches!(stage, "sent" | "settled");
    let mut output_failed = projection_failed;
    if let Some(output) = output.as_ref() {
        match encode(output, format) {
            Ok(bytes) => {
                if stdout.write_all(&bytes).is_err() && failure.is_none() {
                    return dispatch::local(
                        stderr,
                        dispatch::LocalError {
                            code: "RequestFailed",
                            message: "output failed".into(),
                            exit: 1,
                        },
                    );
                }
            }
            Err(_) => {
                output_failed = needs_reconciliation || failure.is_none();
                write_delete_fallback(stdout, format, stage);
            }
        }
    } else {
        write_delete_fallback(stdout, format, stage);
    }
    if output_failed {
        let code = if needs_reconciliation {
            "NotSettled"
        } else {
            "RequestFailed"
        };
        errors::write_error(
            stderr,
            &Envelope {
                code,
                message: "quota output unavailable; reconcile with reads",
                operation: None,
                status: None,
            },
        );
        1
    } else if let Some((error, _)) = failure {
        let mut classified = errors::classify(&error, SERVICE.error_rules);
        if let Some(message) = error.detail::<String>() {
            classified.envelope.message = message;
        }
        errors::write_error(stderr, &classified.envelope);
        classified.exit
    } else {
        0
    }
}

fn write_delete_fallback(stdout: &mut dyn Write, format: Format, stage: &str) {
    if let Ok(projected) = render::project(&fallback_delete(stage))
        && let Ok(bytes) = super::synthetic_writes::outcome::encode(&projected, format)
    {
        let _ = stdout.write_all(&bytes);
    }
}

fn create(input: &Input) -> sdk::CreateNotificationQuotaInput {
    let mut value = sdk::CreateNotificationQuotaInput::new(&input.kind);
    value.package_id.clone_from(&input.package_id);
    value.max_price = input.max_price.unwrap_or_default();
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
    value.max_price = input.max_price.unwrap_or_default();
    value
}
fn synthetic_renew(input: &Input) -> sdk::SetSyntheticQuotaAutoRenewInput {
    let mut value = sdk::SetSyntheticQuotaAutoRenewInput::new(input.enabled.unwrap_or_default());
    value.expected_quota_id.clone_from(&input.expected_quota_id);
    value.max_price = input.max_price.unwrap_or_default();
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
fn delete_preflight(_: &crate::cli::errors::Envelope<'_>) -> Option<render::Projected> {
    render::project(&DeleteEffect {
        quota: None,
        stage: String::new(),
        purged: false,
        effect: EFFECT,
    })
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_delete_table_failure_keeps_send_evidence_and_effect() {
        let mut fields =
            std::collections::BTreeMap::from([("Stage".into(), Value::String("settled".into()))]);
        fields.extend((0..101).map(|index| (format!("column-{index}"), Value::Null)));
        let projected = render::project(&Value::Object(fields)).unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = finish_delete(
            &mut stdout,
            &mut stderr,
            Format::Table,
            Ok(Some(Ok(projected))),
        );
        assert_eq!(code, 1);
        let fallback = render::project(&fallback_delete("sent")).unwrap();
        let expected =
            super::super::synthetic_writes::outcome::encode(&fallback, Format::Table).unwrap();
        assert!(stdout == expected);
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["code"], "NotSettled");
    }

    #[test]
    fn broken_delete_stdout_preserves_the_sdk_error_and_exit_code() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("fixture write refused"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let partial = render::project(&fallback_delete("prepared")).unwrap();
        let failure = dispatch::OperationFailure {
            error: greennode_sdk::Error::provider_failed(),
            partial: Some(partial),
            dispatch: greennode_sdk::WriteDispatch::NotSent,
            query_partial: false,
        };
        let mut stderr = Vec::new();
        let code = finish_delete(&mut Broken, &mut stderr, Format::Json, Err(failure.into()));
        assert_eq!(code, 3);
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["code"], "LoginFailed");
    }

    #[test]
    fn delete_fallback_preserves_a_definitive_auth_refusal() {
        let mut fields =
            std::collections::BTreeMap::from([("Stage".into(), Value::String("prepared".into()))]);
        fields.extend((0..101).map(|index| (format!("column-{index}"), Value::Null)));
        let failure = dispatch::OperationFailure {
            error: greennode_sdk::Error::provider_failed(),
            partial: Some(render::project(&Value::Object(fields)).unwrap()),
            dispatch: greennode_sdk::WriteDispatch::MayHaveBeenSent,
            query_partial: false,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = finish_delete(&mut stdout, &mut stderr, Format::Table, Err(failure.into()));
        assert_eq!(code, 3);
        assert!(String::from_utf8(stdout).unwrap().contains("prepared"));
    }

    #[test]
    fn broken_stdout_after_success_returns_output_error_without_fallback() {
        struct Broken(usize);
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                self.0 += 1;
                Err(std::io::Error::other("fixture output failure"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let projected = render::project(&fallback_delete("settled")).unwrap();
        let mut stdout = Broken(0);
        let mut stderr = Vec::new();
        let code = finish_delete(
            &mut stdout,
            &mut stderr,
            Format::Json,
            Ok(Some(Ok(projected))),
        );
        assert_eq!(code, 1);
        assert_eq!(stdout.0, 1);
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["code"], "RequestFailed");
    }
}
