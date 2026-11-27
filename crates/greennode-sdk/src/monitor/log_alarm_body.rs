use super::mutation::Mutation;
use super::wire::input_error;
use super::{Alarm, AlarmFilter, LogAlarmDetail, LogAlarmResend, ResponseField};
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::{Error, Failure};
use serde_json::{Value, json, value::RawValue};

pub(super) fn body(
    alarm: &Alarm,
    detail: &LogAlarmDetail,
    update: bool,
    send_resend: bool,
) -> Option<Value> {
    let threshold = number(detail.threshold_value)?;
    let threshold_text = crate::output_number::f64(threshold, crate::output_number::Style::Fixed)?;
    let op = match detail.condition.as_str() {
        "gt" => ">",
        "gte" => " >= ",
        "lt" => "<",
        "lte" => " <= ",
        _ => "",
    };
    let label = if detail.threshold_type == "metric_aggregation" {
        format!("{}[{}]", detail.agg_type, detail.agg_field)
    } else {
        "query".into()
    };
    let reason = format!("{label}({}) {op}{threshold_text}", detail.query_string);
    let group = (!detail.group_by_field.is_empty()).then_some(&detail.group_by_field);
    let mut wire = json!({"name":alarm.name,"description":alarm.description,"severity":alarm.severity,
        "logProjectId":detail.log_project_id,"projectName":detail.log_project_name,"zone":"", "queryString":detail.query_string,
        "logSearchQuery":detail.raw_log_search_query,"thresholdType":detail.threshold_type,"condition":detail.condition,
        "thresholdValue":threshold,"timeFrame":number(detail.time_frame)?,"groupByField":group,"reason":reason,
        "inAlarm":join_ids(&detail.in_alarm),"ok":join_ids(&detail.ok),"undetermined":"","resendStatus":detail.resend.statuses.join(","),
    });
    let object = wire.as_object_mut()?;
    if !detail.id.is_empty() {
        object.insert("id".into(), json!(detail.id));
    }
    if update {
        object.insert("logProject".into(), json!(detail.log_project_id));
        object.insert("logProjectName".into(), json!(detail.log_project_name));
    }
    if let Some(filter) = detail.filter.0.as_ref() {
        object.insert("filter".into(), serde_json::from_str(filter.get()).ok()?);
    }
    if !detail.agg_field.is_empty() {
        object.insert("metricAggKey".into(), json!(detail.agg_field));
    }
    if !detail.agg_type.is_empty() {
        object.insert("metricAggType".into(), json!(detail.agg_type));
    }
    if send_resend {
        object.insert(
            "resendEnabled".into(),
            json!(number(detail.resend.enabled)?),
        );
        object.insert("resendPeriod".into(), json!(number(detail.resend.period)?));
        object.insert("resendTimes".into(), json!(number(detail.resend.times)?));
    }
    Some(wire)
}

pub(super) fn validate_request(
    mutation: &Mutation,
    alarm: &Alarm,
    detail: &LogAlarmDetail,
    wire: &Value,
    evidence: &Evidence,
) -> Result<(), Error> {
    if [&alarm.id, &detail.id, &detail.log_project_id]
        .into_iter()
        .chain(&detail.in_alarm)
        .chain(&detail.ok)
        .any(|id| evidence.id_reflects(id))
        || evidence.reflected_in(wire)
    {
        return Err(usage(mutation.id, "request operand withheld"));
    }
    if evidence.text_reflects(
        &crate::output_number::f64(
            number(detail.threshold_value)
                .ok_or_else(|| usage(mutation.id, "ThresholdValue must be finite"))?,
            crate::output_number::Style::Fixed,
        )
        .ok_or_else(|| usage(mutation.id, "ThresholdValue must be finite"))?,
    ) {
        return Err(usage(mutation.id, "request operand withheld"));
    }
    Ok(())
}

pub(super) fn validate_threshold<T>(op: &str, value: f64) -> Result<(), Failure<T>> {
    if !value.is_finite() {
        return Err(super::mutation::not_sent(usage(
            op,
            "ThresholdValue must be a finite number",
        )));
    }
    Ok(())
}

pub(super) fn validate_channels<T>(
    op: &str,
    field: &str,
    ids: &[String],
) -> Result<(), Failure<T>> {
    if ids.iter().any(|id| !PathIds::Strict.allows(id)) {
        return Err(super::mutation::before_write(super::wire::path_id_failure(
            op, field,
        )));
    }
    Ok(())
}

pub(super) fn validate_filter(
    op: &str,
    query: &str,
    filter: Option<&RawValue>,
) -> Result<(), Error> {
    if query.is_empty() != filter.is_none() {
        return Err(usage(
            op,
            "QueryString and Filter must both be set or both left empty",
        ));
    }
    if filter.is_some_and(|raw| {
        !raw.get().starts_with('{')
            || crate::wire::strict_json::validate(raw.get().as_bytes(), &[], raw.get().len())
                .is_err()
    }) {
        return Err(usage(op, "Filter must be a JSON object"));
    }
    Ok(())
}

pub(super) fn default_filter(query: &str) -> Result<AlarmFilter, serde_json::Error> {
    let must = if query.is_empty() {
        Vec::new()
    } else {
        vec![
            json!({"type":"bool","value":{"filter":[],"should":[],"mustNot":[],"must":[{"type":"multi_match","value":{"type":"phrase","query":query,"lenient":true}}]}}),
        ]
    };
    serde_json::value::to_raw_value(
        &json!({"type":"bool","value":{"filter":[],"should":[],"must":must,"mustNot":[]}}),
    )
    .map(|raw| AlarmFilter(Some(raw)))
}
pub(super) fn default_resend() -> LogAlarmResend {
    LogAlarmResend {
        statuses: vec!["ALARM".into()],
        period: ResponseField::Value(30),
        ..Default::default()
    }
}
pub(super) fn number<T: Copy>(value: ResponseField<T>) -> Option<T> {
    match value {
        ResponseField::Value(value) => Some(value),
        ResponseField::Redacted => None,
    }
}
pub(super) fn usage(op: &str, message: &str) -> Error {
    input_error(format!("vngcloud: invalid input: {op}: {message}"))
}
fn join_ids(ids: &[String]) -> String {
    ids.iter().flat_map(|id| [id.as_str(), ","]).collect()
}
