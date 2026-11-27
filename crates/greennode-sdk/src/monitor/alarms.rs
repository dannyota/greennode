use super::inventory::{PAGE, Page, get_input, id, output, page_values, plain, strict};
use super::wire::{check_path_id, failure, requires_failure};
use super::{Client, ResponseField};
use crate::model::wire::{WireField, model};
use crate::wire::product::evidence::Evidence;
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Failure, PagedList, RequestContext};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;

const GET: super::wire::Read = plain("monitor.GetAlarm", "vmonitor-api/api/v1/alarms/{}");
const LIST: super::wire::Read = strict("monitor.ListAlarms", "vmonitor-api/api/v1/alarms/list");
const ENVELOPE: Shape<'static> = Shape::Object(&[ShapeField {
    name: "data",
    shape: Shape::Any,
}]);

get_input!(
    /// Selects one alert rule.
    GetAlarmInput,
    /// Alarm identifier.
    alarm_id
);

/// Selected alert rule.
#[derive(Clone, Debug, Serialize)]
pub struct GetAlarmOutput {
    /// The selected alarm.
    #[serde(rename = "Alarm")]
    pub alarm: Alarm,
}

/// Filters and pagination for alarms.
#[derive(Clone, Debug)]
pub struct ListAlarmsInput {
    /// Provider alarm kind.
    pub kind: String,
    /// Optional name filter.
    pub name: String,
    /// Optional status filter.
    pub status: String,
    /// Optional severity filter.
    pub severity: String,
    /// One-based page. A non-positive value selects the first page.
    pub page: i64,
    /// Requested page size. A non-positive value selects 10000.
    pub size: i64,
}
impl ListAlarmsInput {
    /// Selects the alarm kind to list.
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            name: String::new(),
            status: String::new(),
            severity: String::new(),
            page: 0,
            size: 0,
        }
    }
}
/// One page of matching alarms.
pub type ListAlarmsOutput = PagedList<Alarm>;

/// An account alarm and its supported configuration.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Alarm {
    /// Alarm identifier.
    #[serde(rename = "ID")]
    pub id: String,
    /// Alarm name.
    pub name: String,
    /// Alarm description.
    pub description: String,
    /// Alarm kind.
    pub kind: String,
    /// Current status.
    pub status: String,
    /// Alarm severity.
    pub severity: String,
    /// Metric channel mapping identifier.
    #[serde(rename = "MetricMappingID")]
    pub metric_mapping_id: String,
    /// Log-only configuration.
    pub log: Option<LogAlarmDetail>,
}

/// Settings for a log alarm.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct LogAlarmDetail {
    /// Log alarm configuration identifier.
    #[serde(rename = "ID")]
    pub id: String,
    /// Log project identifier.
    #[serde(rename = "LogProjectID")]
    pub log_project_id: String,
    /// Log project name.
    pub log_project_name: String,
    /// Log search query.
    pub query_string: String,
    /// Provider filter.
    pub filter: AlarmFilter,
    /// Threshold kind.
    pub threshold_type: String,
    /// Condition for raising the alarm.
    pub condition: String,
    /// Configured threshold.
    pub threshold_value: ResponseField<f64>,
    /// Evaluation window.
    pub time_frame: ResponseField<i64>,
    /// Log field used for grouping.
    pub group_by_field: String,
    /// Log field used for aggregation.
    pub agg_field: String,
    /// Aggregation kind.
    pub agg_type: String,
    /// Channel IDs notified on entering alarm.
    pub in_alarm: Vec<String>,
    /// Channel IDs notified on leaving alarm.
    #[serde(rename = "OK")]
    pub ok: Vec<String>,
    /// Notification resend settings.
    pub resend: LogAlarmResend,
}

/// Settings for repeating notifications.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct LogAlarmResend {
    /// Whether notifications are resent.
    pub enabled: ResponseField<bool>,
    /// States whose notifications are resent.
    pub statuses: Vec<String>,
    /// Minutes between notifications.
    pub period: ResponseField<i64>,
    /// Number of repetitions.
    pub times: ResponseField<i64>,
}

/// Raw provider JSON for a log alarm filter.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(transparent)]
pub struct AlarmFilter(pub Option<Box<RawValue>>);

impl PartialEq for AlarmFilter {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_deref().map(RawValue::get) == other.0.as_deref().map(RawValue::get)
    }
}
impl Serialize for AlarmFilter {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.0 {
            Some(value) => value.serialize(serializer),
            None => Vec::<()>::new().serialize(serializer),
        }
    }
}
impl WireField for AlarmFilter {
    fn redact(&mut self, evidence: &Evidence) -> bool {
        let Some(raw) = self.0.as_ref() else {
            return !evidence.text_reflects("[]");
        };
        let Some(clean) = super::raw_json::redact(raw.get(), evidence) else {
            return false;
        };
        if clean != raw.get() {
            let Ok(safe) = RawValue::from_string(clean) else {
                return false;
            };
            self.0 = Some(safe);
        }
        true
    }
}

#[derive(Clone, Default, Debug, PartialEq, Serialize)]
struct ScalarText(String);
impl<'de> Deserialize<'de> for ScalarText {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(d)?;
        let value = match raw.get().as_bytes().first() {
            Some(b'"') => serde_json::from_str(raw.get()).unwrap_or_default(),
            Some(b'-' | b'0'..=b'9') => raw.get().into(),
            _ => String::new(),
        };
        Ok(Self(value))
    }
}
impl WireField for ScalarText {
    fn redact(&mut self, evidence: &Evidence) -> bool {
        self.0.redact(evidence)
    }
}

#[derive(Clone, Default, Debug, PartialEq, Serialize)]
struct FlexibleNumber(f64);
impl<'de> Deserialize<'de> for FlexibleNumber {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(d)?;
        let unquoted;
        let text = if raw.get().starts_with('"') {
            unquoted = serde_json::from_str::<String>(raw.get()).unwrap_or_default();
            unquoted.as_str()
        } else {
            raw.get()
        };
        Ok(Self(
            crate::go_float::parse_go_float(text).unwrap_or_default(),
        ))
    }
}
impl WireField for FlexibleNumber {
    fn redact(&mut self, evidence: &Evidence) -> bool {
        self.0.redact(evidence)
    }
}

model! { struct AlarmWire {
    id: ScalarText => "id", "ID";
    name: String => "name", "Name";
    description: String => "description", "Description";
    kind: String => "type", "Kind";
    status: String => "status", "Status";
    progress: String => "progressStatus", "ProgressStatus";
    severity: String => "severity", "Severity";
    mapping: String => "metricMappingId", "MetricMappingID";
    in_alarm: Option<String> => "inAlarm", "InAlarm";
    ok: Option<String> => "ok", "OK";
    log: Option<LogWire> => "alarmLog", "Log";
}}
model! { struct LogWire {
    id: ScalarText => "id", "ID";
    project: ScalarText => "logProject", "LogProjectID";
    name: String => "logProjectName", "LogProjectName";
    query: String => "queryString", "QueryString";
    _search_query: String => "logSearchQuery", "LogSearchQuery";
    filter: AlarmFilter => "filter", "Filter";
    threshold_type: String => "thresholdType", "ThresholdType";
    condition: String => "condition", "Condition";
    threshold: FlexibleNumber => "thresholdValue", "ThresholdValue";
    time_frame: FlexibleNumber => "timeFrame", "TimeFrame";
    group: ScalarText => "groupByField", "GroupByField";
    agg_field: String => "metricAggKey", "AggField";
    agg_type: String => "metricAggType", "AggType";
    in_alarm: Option<String> => "inAlarm", "InAlarm";
    ok: Option<String> => "ok", "OK";
    resend_enabled: bool => "resendEnabled", "ResendEnabled";
    resend_status: String => "resendStatus", "ResendStatus";
    resend_period: FlexibleNumber => "resendPeriod", "ResendPeriod";
    resend_times: FlexibleNumber => "resendTimes", "ResendTimes";
}}
model! { struct AlarmEnvelope {data: Alarm => "data", "Alarm";} }

impl<'de> Deserialize<'de> for Alarm {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let wire = AlarmWire::deserialize(d)?;
        let kind = if wire.kind.eq_ignore_ascii_case("log") {
            "Log"
        } else if wire.kind.eq_ignore_ascii_case("metric") {
            "Metric"
        } else {
            ""
        };
        let log = match wire.log {
            Some(wire) => Some(wire.into()),
            None if wire.in_alarm.is_some() || wire.ok.is_some() => Some(LogAlarmDetail {
                in_alarm: split(wire.in_alarm.as_deref()),
                ok: split(wire.ok.as_deref()),
                ..Default::default()
            }),
            None => None,
        };
        Ok(Self {
            id: wire.id.0,
            name: wire.name,
            description: wire.description,
            kind: kind.into(),
            status: if wire.progress.is_empty() {
                wire.status
            } else {
                wire.progress
            },
            severity: wire.severity,
            metric_mapping_id: wire.mapping,
            log,
        })
    }
}
impl From<LogWire> for LogAlarmDetail {
    fn from(wire: LogWire) -> Self {
        Self {
            id: wire.id.0,
            log_project_id: wire.project.0,
            log_project_name: wire.name,
            query_string: wire.query,
            filter: wire.filter,
            threshold_type: wire.threshold_type,
            condition: wire.condition,
            threshold_value: ResponseField::Value(wire.threshold.0),
            time_frame: ResponseField::Value(wire.time_frame.0 as i64),
            group_by_field: wire.group.0,
            agg_field: wire.agg_field,
            agg_type: wire.agg_type,
            in_alarm: split(wire.in_alarm.as_deref()),
            ok: split(wire.ok.as_deref()),
            resend: LogAlarmResend {
                enabled: ResponseField::Value(wire.resend_enabled),
                statuses: split(Some(&wire.resend_status)),
                period: ResponseField::Value(wire.resend_period.0 as i64),
                times: ResponseField::Value(wire.resend_times.0 as i64),
            },
        }
    }
}
fn split(text: Option<&str>) -> Vec<String> {
    text.unwrap_or_default()
        .split(',')
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}
impl WireField for Alarm {
    fn redact(&mut self, evidence: &Evidence) -> bool {
        id(&mut self.id, evidence)
            && id(&mut self.metric_mapping_id, evidence)
            && [
                &mut self.name,
                &mut self.description,
                &mut self.kind,
                &mut self.status,
                &mut self.severity,
            ]
            .into_iter()
            .all(|value| value.redact(evidence))
            && self.log.as_mut().map_or_else(
                || !evidence.text_reflects("null"),
                |log| log.redact(evidence),
            )
    }
}
impl LogAlarmDetail {
    fn redact(&mut self, evidence: &Evidence) -> bool {
        id(&mut self.id, evidence)
            && id(&mut self.log_project_id, evidence)
            && self
                .in_alarm
                .iter_mut()
                .chain(&mut self.ok)
                .all(|value| id(value, evidence))
            && [
                &mut self.log_project_name,
                &mut self.query_string,
                &mut self.threshold_type,
                &mut self.condition,
                &mut self.group_by_field,
                &mut self.agg_field,
                &mut self.agg_type,
            ]
            .into_iter()
            .all(|value| value.redact(evidence))
            && self.filter.redact(evidence)
            && (matches!(self.threshold_value, ResponseField::Value(value) if !value.is_finite())
                || self.threshold_value.redact(evidence))
            && self.time_frame.redact(evidence)
            && self.resend.enabled.redact(evidence)
            && self.resend.statuses.redact(evidence)
            && self.resend.period.redact(evidence)
            && self.resend.times.redact(evidence)
    }
}

impl Client {
    /// Reads one alert rule.
    pub async fn get_alarm(
        &self,
        ctx: &RequestContext,
        input: GetAlarmInput,
    ) -> Result<GetAlarmOutput, Failure<GetAlarmOutput>> {
        check_path_id(GET.id, "AlarmID", &input.alarm_id)?;
        let mut evidence = Evidence::new();
        let raw = self
            .read(ctx, &GET, &[&input.alarm_id], &[], &mut evidence)
            .await
            .map_err(failure)?;
        let mut envelope: AlarmEnvelope =
            output::<Option<AlarmEnvelope>>(&raw, &ENVELOPE, &GET, &evidence)
                .map_err(failure)?
                .unwrap_or_default();
        if !envelope.data.redact(&evidence) {
            return Err(failure(GET.invalid(&evidence)));
        }
        Ok(GetAlarmOutput {
            alarm: envelope.data,
        })
    }
    /// Lists alerts of the requested kind.
    pub async fn list_alarms(
        &self,
        ctx: &RequestContext,
        input: ListAlarmsInput,
    ) -> Result<ListAlarmsOutput, Failure<ListAlarmsOutput>> {
        if input.kind.is_empty() {
            return Err(requires_failure(LIST.id, "Kind"));
        }
        let mut evidence = Evidence::new();
        let (page, size) = page_values(input.page, input.size, 1, 10000);
        let query = [
            ("type-alarm", input.kind.as_str()),
            ("name", input.name.as_str()),
            ("status", input.status.as_str()),
            ("severity", input.severity.as_str()),
            ("page", page.as_str()),
            ("size", size.as_str()),
        ];
        let raw = self
            .read(ctx, &LIST, &[], &query, &mut evidence)
            .await
            .map_err(failure)?;
        let page: Page<Alarm> = output(&raw, &PAGE, &LIST, &evidence).map_err(failure)?;
        let mut page = page.into_page(true, &LIST, &evidence).map_err(failure)?;
        for item in &mut page.items {
            item.kind.clone_from(&input.kind);
            match input.kind.as_str() {
                "Log" => item.metric_mapping_id.clear(),
                "Metric" => item.log = None,
                _ => (),
            }
            if !item.redact(&evidence) {
                return Err(failure(LIST.invalid(&evidence)));
            }
        }
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::{Alarm, AlarmFilter, FlexibleNumber};
    use crate::Secret;
    use crate::model::wire::WireField;
    use crate::wire::product::evidence::Evidence;

    #[test]
    fn filter_redacts_string_without_changing_number_spellings() {
        let raw = r#"{"z":1e5, "s":"secret-token", "a":0.10}"#;
        let mut filter: AlarmFilter = serde_json::from_str(raw).unwrap();
        let mut evidence = Evidence::new();
        evidence.add_secret(&Secret::new("secret-token"));
        assert!(filter.redact(&evidence));
        assert_eq!(
            filter.0.unwrap().get(),
            r#"{"z":1e5, "s":"<redacted>", "a":0.10}"#
        );
    }

    #[test]
    fn filter_redacts_raw_numbers_without_changing_other_bytes() {
        let raw = r#"{"z":100000000000000000000000000000, "a":12345, "b":1e+30}"#;
        let mut filter: AlarmFilter = serde_json::from_str(raw).unwrap();
        let mut evidence = Evidence::new();
        evidence.add_secret(&Secret::new("12345"));
        assert!(filter.redact(&evidence));
        assert_eq!(
            filter.0.unwrap().get(),
            r#"{"z":100000000000000000000000000000, "a":"<redacted>", "b":1e+30}"#
        );
    }

    #[test]
    fn log_search_query_is_checked_for_type_and_alias_collisions() {
        assert!(serde_json::from_str::<Alarm>(r#"{"alarmLog":{"logSearchQuery":42}}"#).is_err());
        assert!(
            serde_json::from_str::<Alarm>(
                r#"{"alarmLog":{"logSearchQuery":"a","LogSearchQuery":"b"}}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<Alarm>(r#"{"alarmLog":{"logSearchQuery":"safe"}}"#).is_ok());
    }

    #[test]
    fn threshold_numbers_follow_go_float_parsing() {
        assert_eq!(
            serde_json::from_str::<FlexibleNumber>(r#""0x1.8p2""#)
                .unwrap()
                .0,
            6.0
        );
        assert!(
            serde_json::from_str::<FlexibleNumber>(r#""NaN""#)
                .unwrap()
                .0
                .is_nan()
        );
        assert!(
            serde_json::from_str::<FlexibleNumber>(r#""+Inf""#)
                .unwrap()
                .0
                .is_infinite()
        );
        assert_eq!(
            serde_json::from_str::<FlexibleNumber>(r#""1e999""#)
                .unwrap()
                .0,
            0.0
        );
    }

    #[test]
    fn omitted_filter_serializes_as_empty_array() {
        assert_eq!(
            serde_json::to_string(&AlarmFilter::default()).unwrap(),
            "[]"
        );
    }
}
