use std::io::Write;

use greennode_sdk::monitor::{QuotaMutationOutcome, ResponseField};
use greennode_sdk::{Failure, WriteDispatch};
use serde::{Serialize, Serializer, ser::SerializeStruct};

use crate::cli::dispatch::{ExecutionFailure, ExecutionResult, Format, OperationFailure};
use crate::cli::{errors, render};
use render::Value;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Create,
    Renew,
    Recover,
    Ensure,
}

#[derive(Clone, Copy)]
pub(super) struct Shape {
    pub(super) operation: &'static str,
    pub(super) action: Action,
}

#[derive(Clone, Copy)]
struct Meta<'a> {
    code: &'a str,
    status: Option<u16>,
}

impl Meta<'_> {
    const FAILED: Self = Self {
        code: "RequestFailed",
        status: None,
    };
    const UNSETTLED: Self = Self {
        code: "NotSettled",
        status: None,
    };
}

impl<'a> From<&'a errors::Envelope<'_>> for Meta<'a> {
    fn from(envelope: &'a errors::Envelope<'_>) -> Self {
        Self {
            code: envelope.code,
            status: envelope.status,
        }
    }
}

type Fields = std::collections::BTreeMap<String, Value>;

pub(super) trait Output: Serialize {
    fn outcome(&self) -> &QuotaMutationOutcome;
    fn fallback_fields(&self) -> Fields;
}

macro_rules! output {
    ($name:ident, $time:ident, $key:literal $(, $action:ident)?) => {
        impl Output for greennode_sdk::monitor::$name {
            fn outcome(&self) -> &QuotaMutationOutcome { &self.outcome }
            fn fallback_fields(&self) -> Fields {
                #[allow(unused_mut)]
                let mut fields = Fields::from([
                    ("OrderID".into(), Value::String(self.order_id.clone())),
                    ("PaymentPending".into(), Value::Bool(self.payment_pending)),
                    ($key.into(), render::project(&self.$time).and_then(|value| value.for_query()).unwrap_or(Value::Null)),
                ]);
                $(fields.insert("Action".into(), Value::String(self.$action.clone()));)?
                fields
            }
        }
    };
}
output!(CreateSyntheticQuotaOutput, sent_at, "SentAt");
output!(
    RenewSyntheticQuotaOutput,
    previous_end_time,
    "PreviousEndTime"
);
output!(
    RecoverSyntheticQuotaOutput,
    previous_end_time,
    "PreviousEndTime"
);
output!(
    EnsureSyntheticQuotaOutput,
    previous_end_time,
    "PreviousEndTime",
    action
);

impl Shape {
    pub(super) fn preflight(self, envelope: &errors::Envelope<'_>) -> Option<render::Projected> {
        self.fallback(None, "no", envelope.into())
    }

    #[expect(
        clippy::result_large_err,
        reason = "ExecutionResult carries the classified SDK failure"
    )]
    pub(super) fn project<T: Output>(self, result: Result<T, Failure<T>>) -> ExecutionResult {
        let encoding_fallback = |value: &T| {
            let evidence = value.outcome();
            let meta = evidence.error.as_ref().map_or(Meta::FAILED, |error| Meta {
                code: &error.code,
                status: match error.status {
                    ResponseField::Value(value) => u16::try_from(value).ok(),
                    ResponseField::Redacted => None,
                },
            });
            self.fallback(Some(&value.fallback_fields()), &evidence.sent, meta)
        };
        match result {
            Ok(value) => match render::project(&value) {
                Ok(projected) => Ok(Some(Ok(projected))),
                Err(_) => Err(ExecutionFailure::Projection(encoding_fallback(&value))),
            },
            Err(failure) => {
                let (error, partial, dispatch) = failure.into_parts();
                Err(OperationFailure {
                    error,
                    partial: partial.as_ref().and_then(|value| {
                        render::project(value)
                            .ok()
                            .or_else(|| encoding_fallback(value))
                    }),
                    dispatch,
                    query_partial: false,
                }
                .into())
            }
        }
    }

    pub(super) fn finish(
        self,
        stdout: &mut dyn Write,
        stderr: &mut dyn Write,
        format: Format,
        result: ExecutionResult,
    ) -> u8 {
        let (projected, sdk_failure, default) = match result {
            Ok(Some(Ok(projected))) => (Some(projected), None, None),
            Ok(_) => (None, None, Some((Meta::FAILED, 1))),
            Err(ExecutionFailure::Local(error)) => (
                None,
                None,
                Some((
                    Meta {
                        code: error.code,
                        status: None,
                    },
                    error.exit,
                )),
            ),
            Err(ExecutionFailure::Projection(fallback)) => {
                (fallback, None, Some((Meta::FAILED, 1)))
            }
            Err(ExecutionFailure::Sdk(failure)) => {
                let OperationFailure {
                    error,
                    partial,
                    dispatch,
                    ..
                } = failure;
                (partial, Some((error, dispatch)), None)
            }
        };
        let classified = sdk_failure
            .as_ref()
            .map(|(error, _)| errors::classify(error, super::super::SERVICE.error_rules));
        let fields = projected
            .as_ref()
            .and_then(|value| match value.for_query().ok()? {
                Value::Object(fields) => Some(fields),
                _ => None,
            });
        let meta = fields.as_ref().and_then(outcome_meta);
        let default = default.or_else(|| {
            classified
                .as_ref()
                .map(|value| ((&value.envelope).into(), value.exit))
        });
        let no_projection = meta
            .or_else(|| default.map(|(meta, _)| meta))
            .unwrap_or(Meta::FAILED);
        if let Some(projected) = projected.as_ref()
            && let Ok(buffer) = encode(projected, format)
        {
            let wrote = stdout.write_all(&buffer).is_ok();
            if let Some((failure, exit)) = default {
                let meta = meta.unwrap_or(failure);
                self.write_error(stderr, meta.code, meta.status);
                return exit;
            }
            if wrote {
                return 0;
            }
            self.write_error(stderr, Meta::FAILED.code, None);
            return 1;
        }
        let sent = fields
            .as_ref()
            .and_then(|fields| text(fields, "Sent"))
            .unwrap_or_else(
                || match sdk_failure.as_ref().map(|(_, dispatch)| dispatch) {
                    Some(WriteDispatch::MayHaveBeenSent) if no_projection.status.is_some() => "yes",
                    Some(WriteDispatch::MayHaveBeenSent) => "unknown",
                    _ => "no",
                },
            );
        let fallback_error = if sent == "no" {
            no_projection
        } else {
            Meta {
                status: no_projection.status,
                ..Meta::UNSETTLED
            }
        };
        if let Some(output) = self.fallback(fields.as_ref(), sent, fallback_error)
            && let Ok(buffer) = encode(&output, format)
        {
            let _ = stdout.write_all(&buffer);
        }
        self.write_error(stderr, fallback_error.code, fallback_error.status);
        if sent == "no" {
            default.map_or(1, |(_, exit)| exit)
        } else {
            1
        }
    }

    fn fallback(
        self,
        fields: Option<&Fields>,
        sent: &str,
        meta: Meta<'_>,
    ) -> Option<render::Projected> {
        render::project(&Fallback {
            shape: self,
            fields,
            sent,
            meta,
        })
        .ok()
    }

    pub(super) fn write_error(self, stderr: &mut dyn Write, code: &str, status: Option<u16>) {
        errors::write_error_operation_first(
            stderr,
            &errors::Envelope {
                code,
                message: "quota command failed; inspect the outcome and reconcile with reads before another write",
                operation: Some(self.operation),
                status: Some(status.unwrap_or_default()),
            },
        );
    }
}

fn text<'a>(fields: &'a Fields, name: &str) -> Option<&'a str> {
    match fields.get(name)? {
        Value::String(value) => Some(value),
        _ => None,
    }
}

fn outcome_meta(fields: &Fields) -> Option<Meta<'_>> {
    let Value::Object(error) = fields.get("Error")? else {
        return None;
    };
    let status = match error.get("Status") {
        Some(Value::Number(value)) => value.to_string().parse().ok(),
        _ => None,
    };
    Some(Meta {
        code: text(error, "Code")?,
        status,
    })
}

struct Fallback<'a> {
    shape: Shape,
    fields: Option<&'a Fields>,
    sent: &'a str,
    meta: Meta<'a>,
}

impl Serialize for Fallback<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "PascalCase")]
        struct Detail<'a> {
            code: &'a str,
            operation: &'static str,
            status: u16,
        }
        let action = self.shape.action;
        let mut output = serializer.serialize_struct(
            "QuotaMutation",
            if action == Action::Ensure { 10 } else { 8 },
        )?;
        let lookup = |key| self.fields.and_then(|fields| fields.get(key));
        let previous = lookup("PreviousEndTime");
        let order_id = lookup("OrderID").unwrap_or(&Value::Null);
        let pending = matches!(lookup("PaymentPending"), Some(Value::Bool(true)));
        output.serialize_field("Sent", self.sent)?;
        output.serialize_field(
            "Stage",
            if self.sent == "no" {
                "preflight"
            } else {
                "unconfirmed"
            },
        )?;
        output.serialize_field(
            "Error",
            &Detail {
                code: if self.sent == "no" {
                    self.meta.code
                } else {
                    "NotSettled"
                },
                operation: self.shape.operation,
                status: self.meta.status.unwrap_or_default(),
            },
        )?;
        if action == Action::Ensure {
            output.serialize_field(
                "Action",
                &lookup("Action").unwrap_or(&Value::String("none".into())),
            )?;
        }
        if matches!(action, Action::Renew | Action::Recover) {
            output.serialize_field("PreviousEndTime", &previous)?;
        }
        output.serialize_field("Quota", &Value::Null)?;
        match action {
            Action::Create => output.serialize_field("Created", &false)?,
            Action::Ensure => {
                output.serialize_field("PreviousEndTime", &previous)?;
                output.serialize_field("Renewed", &false)?;
                output.serialize_field("Recovered", &false)?;
            }
            _ => (),
        }
        if let Value::String(id) = order_id {
            output.serialize_field("OrderID", id)?;
        } else {
            output.serialize_field("OrderID", "")?;
        }
        output.serialize_field("PaymentPending", &pending)?;
        match action {
            Action::Create => output.serialize_field("SentAt", &lookup("SentAt"))?,
            Action::Renew => output.serialize_field("Renewed", &false)?,
            Action::Recover => output.serialize_field("Recovered", &false)?,
            Action::Ensure => (),
        }
        output.end()
    }
}

pub(in crate::services::monitor) fn encode(
    projected: &render::Projected,
    format: Format,
) -> Result<Vec<u8>, render::Error> {
    let mut buffer = Vec::new();
    match format {
        Format::Json => render::write_json(&mut buffer, projected)?,
        Format::Table => render::write_table(&mut buffer, &projected.for_query()?)?,
        Format::Text => render::write_text(&mut buffer, &projected.for_query()?)?,
    }
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RENEW: Shape = Shape {
        operation: "monitor.RenewSyntheticQuota",
        action: Action::Renew,
    };

    #[test]
    fn encoding_fallback_preserves_dispatched_evidence() {
        struct Invalid(QuotaMutationOutcome);
        impl Serialize for Invalid {
            fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("test encoding failure"))
            }
        }
        impl Output for Invalid {
            fn outcome(&self) -> &QuotaMutationOutcome {
                &self.0
            }
            fn fallback_fields(&self) -> Fields {
                Fields::from([
                    ("OrderID".into(), Value::String("fixture-order".into())),
                    ("PaymentPending".into(), Value::Bool(true)),
                    (
                        "PreviousEndTime".into(),
                        Value::String("2099-02-01T00:00:00Z".into()),
                    ),
                ])
            }
        }
        let outcome = QuotaMutationOutcome {
            sent: "yes".into(),
            stage: "confirmed".into(),
            error: None,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = RENEW.finish(
            &mut stdout,
            &mut stderr,
            Format::Json,
            RENEW.project(Ok(Invalid(outcome))),
        );
        assert_eq!(code, 1);
        let output: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(output["Sent"], "yes");
        assert_eq!(output["Stage"], "unconfirmed");
        assert_eq!(output["Error"]["Code"], "NotSettled");
        assert_eq!(output["Renewed"], false);
        assert_eq!(output["OrderID"], "fixture-order");
        assert_eq!(output["PaymentPending"], true);
        assert_eq!(output["PreviousEndTime"], "2099-02-01T00:00:00Z");
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["code"], "NotSettled");
    }

    #[test]
    fn io_failure_does_not_write_a_fallback_after_partial_stdout() {
        struct Broken(usize);
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                self.0 += 1;
                if self.0 == 1 {
                    Ok(1)
                } else {
                    Err(std::io::Error::other("test output failure"))
                }
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let output = render::project(&QuotaMutationOutcome {
            sent: "yes".into(),
            stage: "confirmed".into(),
            error: None,
        })
        .unwrap();
        let mut stdout = Broken(0);
        let mut stderr = Vec::new();
        let code = RENEW.finish(&mut stdout, &mut stderr, Format::Json, Ok(Some(Ok(output))));
        assert_eq!(code, 1);
        assert_eq!(stdout.0, 2);
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["code"], "RequestFailed");
    }
}
