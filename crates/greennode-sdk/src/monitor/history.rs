use super::check_alarm::timestamp;
use super::response_times::{milliseconds, safe_time, serialize_time};
use super::wire::{Family, Read, check_path_id, decode, failure};
use super::{Client, ResponseField};
use crate::model::wire::{Field, WireField};
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Error, ErrorClass, Failure, RequestContext};
use serde::{Deserialize, Serialize, Serializer};
use std::sync::Arc;
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

const HISTORY: Read = Read {
    id: "monitor.ListCheckAlarmHistory",
    route: "vmonitor-api/api/v1/alarms/synthetic/metrics/mona/{}/histories",
    limit: 16 << 20,
    family: Family::CheckAlarm,
};

/// Selects a check alarm history window and optional location.
#[derive(Clone, Debug)]
pub struct ListCheckAlarmHistoryInput {
    /// Check identifier.
    pub check_id: String,
    /// RFC 3339 start. Empty selects 24 hours before the end.
    pub start: String,
    /// RFC 3339 end. Empty selects now.
    pub end: String,
    /// Optional probe location identifier.
    pub location_id: String,
}

impl ListCheckAlarmHistoryInput {
    /// Selects an alarm history by its check identifier.
    pub fn new(check_id: impl Into<String>) -> Self {
        Self {
            check_id: check_id.into(),
            start: String::new(),
            end: String::new(),
            location_id: String::new(),
        }
    }
}

/// Observed transitions and the requested history window.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ListCheckAlarmHistoryOutput {
    /// Returned transitions sorted by time, location and identifiers.
    pub items: Vec<CheckAlarmTransition>,
    /// Start of the requested window.
    #[serde(serialize_with = "serialize_time")]
    pub start: OffsetDateTime,
    /// End of the requested window.
    #[serde(serialize_with = "serialize_time")]
    pub end: OffsetDateTime,
}

/// One observed location alarm transition.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CheckAlarmTransition {
    /// Transition identifier.
    #[serde(rename = "ID")]
    pub id: String,
    /// Location alarm identifier.
    #[serde(rename = "AlarmID", serialize_with = "shared_text")]
    pub alarm_id: Arc<str>,
    /// Probe location identifier.
    #[serde(rename = "LocationID", serialize_with = "shared_text")]
    pub location_id: Arc<str>,
    /// Probe location name.
    #[serde(serialize_with = "shared_text")]
    pub location_name: Arc<str>,
    /// Previous alarm state.
    pub old_state: String,
    /// New alarm state.
    pub new_state: String,
    /// Transition time.
    #[serde(serialize_with = "serialize_time")]
    pub timestamp: OffsetDateTime,
    /// Sub-alarm observations.
    pub sub_alarms: Vec<CheckSubAlarmState>,
}

/// Reported state and samples for a sub-alarm.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CheckSubAlarmState {
    /// Sub-alarm state.
    pub state: String,
    /// Values reported in the transition.
    pub current_values: Vec<ResponseField<f64>>,
}

#[derive(Deserialize)]
struct History {
    data: Vec<Transition>,
}
#[derive(Deserialize)]
struct Transition {
    id: String,
    alarm_id: String,
    old_state: String,
    new_state: String,
    timestamp: String,
    sub_alarms: Vec<SubAlarm>,
}
#[derive(Deserialize)]
struct SubAlarm {
    sub_alarm_state: String,
    current_values: Vec<Field<Option<f64>>>,
}

macro_rules! obj {
    ($($key:literal $(=> $shape:expr)?),+) => { Shape::Object(&[$(ShapeField { name: $key, shape: obj!(@shape $($shape)?) }),+]) };
    (@shape $shape:expr) => { $shape };
    (@shape) => { Shape::Any };
}
const SUB: Shape<'static> = obj!("sub_alarm_state", "current_values");
const TRANSITION: Shape<'static> = obj!("id", "alarm_id", "old_state", "new_state", "timestamp", "sub_alarms" => Shape::Array(&SUB));
const HISTORY_SHAPE: Shape<'static> = obj!("data" => Shape::Array(&TRANSITION));

impl Client {
    /// Reads every reported alarm transition for the selected check and window.
    pub async fn list_check_alarm_history(
        &self,
        ctx: &RequestContext,
        input: ListCheckAlarmHistoryInput,
    ) -> Result<ListCheckAlarmHistoryOutput, Failure<ListCheckAlarmHistoryOutput>> {
        check_path_id(HISTORY.id, "CheckID", &input.check_id)?;
        if !input.location_id.is_empty() {
            check_path_id(HISTORY.id, "LocationID", &input.location_id)?;
        }
        self.check_alarm_history(ctx, input).await.map_err(failure)
    }

    async fn check_alarm_history(
        &self,
        ctx: &RequestContext,
        input: ListCheckAlarmHistoryInput,
    ) -> Result<ListCheckAlarmHistoryOutput, Error> {
        let mut evidence = Evidence::new();
        let invalid_range = || {
            Error::new(ErrorClass::InvalidInput).with_cause(HISTORY.error(
                None,
                "InvalidInput",
                "Start and End must form a nonempty RFC 3339 range at millisecond precision",
                false,
                &evidence,
            ))
        };
        let end = history_endpoint(&input.end, || Some(self.session.clock().utc_now()))
            .ok_or_else(invalid_range)?;
        let start = history_endpoint(&input.start, || end.checked_sub(Duration::DAY))
            .ok_or_else(invalid_range)?;
        if milliseconds(start) >= milliseconds(end) {
            return Err(invalid_range());
        }
        let (alarm, frequency) = self
            .read_alarm(ctx, HISTORY.id, &input.check_id, &mut evidence)
            .await?;
        let locations: Vec<_> = alarm
            .locations
            .into_iter()
            .filter(|location| {
                input.location_id.is_empty() || input.location_id == location.location_id
            })
            .collect();
        if locations.is_empty() && !input.location_id.is_empty() {
            return Err(Error::new(ErrorClass::NotFound).with_cause(HISTORY.error(
                None,
                "NotFound",
                "check location not found",
                false,
                &evidence,
            )));
        }
        let mut remaining = 32 << 20;
        let mut items = Vec::new();
        let mut names = Vec::new();
        let (from, to, interval) = (
            milliseconds(start).to_string(),
            milliseconds(end).to_string(),
            frequency.to_string(),
        );
        let query = [
            ("start_time", from.as_str()),
            ("end_time", to.as_str()),
            ("interval", interval.as_str()),
        ];
        for location in locations {
            if remaining == 0 {
                return Err(HISTORY.error(
                    Some(200),
                    "ResponseLimitExceeded",
                    "check alarm response rejected; body withheld",
                    false,
                    &evidence,
                ));
            }
            if evidence.id_reflects(&location.alarm_id)
                || evidence.id_reflects(&location.location_id)
            {
                return Err(HISTORY.invalid(&evidence));
            }
            let read = Read {
                limit: HISTORY.limit.min(remaining),
                ..HISTORY
            };
            let raw = self
                .read(ctx, &read, &[&location.alarm_id], &query, &mut evidence)
                .await?;
            remaining = remaining.saturating_sub(raw.get().len());
            let history: History = decode(&raw, &HISTORY_SHAPE, &read, &evidence)?;
            let alarm_id: Arc<str> = location.alarm_id.into();
            let location_id: Arc<str> = location.location_id.into();
            let name_index = names.len();
            names.push(location.location_name);
            for row in history.data {
                let item = transition(row, &alarm_id, &location_id, &evidence)
                    .ok_or_else(|| read.invalid(&evidence))?;
                items.push((name_index, item));
            }
        }
        if evidence.id_reflects(&input.check_id)
            || !safe_time(start, &evidence)
            || !safe_time(end, &evidence)
        {
            return Err(HISTORY.invalid(&evidence));
        }
        if !names.iter_mut().all(|name| name.redact(&evidence)) {
            return Err(HISTORY.invalid(&evidence));
        }
        let names: Vec<Arc<str>> = names.into_iter().map(Arc::from).collect();
        let mut output = Vec::with_capacity(items.len());
        for (index, mut item) in items {
            if evidence.id_reflects(&item.id)
                || evidence.id_reflects(&item.alarm_id)
                || evidence.id_reflects(&item.location_id)
                || !safe_time(item.timestamp, &evidence)
                || !item.old_state.redact(&evidence)
                || !item.new_state.redact(&evidence)
                || !item
                    .sub_alarms
                    .iter_mut()
                    .all(|sub| sub.state.redact(&evidence) && sub.current_values.redact(&evidence))
            {
                return Err(HISTORY.invalid(&evidence));
            }
            item.location_name = names
                .get(index)
                .cloned()
                .ok_or_else(|| HISTORY.invalid(&evidence))?;
            output.push(item);
        }
        output.sort_by(|left, right| {
            (left.timestamp, &left.location_id, &left.alarm_id, &left.id).cmp(&(
                right.timestamp,
                &right.location_id,
                &right.alarm_id,
                &right.id,
            ))
        });
        Ok(ListCheckAlarmHistoryOutput {
            items: output,
            start,
            end,
        })
    }
}

fn history_endpoint(
    raw: &str,
    default: impl FnOnce() -> Option<OffsetDateTime>,
) -> Option<OffsetDateTime> {
    if raw.is_empty() {
        return default();
    }
    if raw.contains(['t', 'z']) {
        return None;
    }
    OffsetDateTime::parse(&raw.replace(',', "."), &Rfc3339).ok()
}

fn transition(
    row: Transition,
    alarm_id: &Arc<str>,
    location_id: &Arc<str>,
    evidence: &Evidence,
) -> Option<CheckAlarmTransition> {
    if row.alarm_id != **alarm_id
        || (!PathIds::WithUnderscore.allows(&row.id) || evidence.id_reflects(&row.id))
        || row.old_state.is_empty()
        || row.new_state.is_empty()
    {
        return None;
    }
    let sub_alarms = row
        .sub_alarms
        .into_iter()
        .map(|sub| {
            if sub.sub_alarm_state.is_empty() {
                return None;
            }
            let values = sub
                .current_values
                .into_iter()
                .map(|value| value.0.map(ResponseField::Value))
                .collect::<Option<_>>()?;
            Some(CheckSubAlarmState {
                state: sub.sub_alarm_state,
                current_values: values,
            })
        })
        .collect::<Option<_>>()?;
    Some(CheckAlarmTransition {
        id: row.id,
        alarm_id: alarm_id.clone(),
        location_id: location_id.clone(),
        location_name: Arc::from(""),
        old_state: row.old_state,
        new_state: row.new_state,
        timestamp: timestamp(&row.timestamp, evidence)?,
        sub_alarms,
    })
}

fn shared_text<S: Serializer>(value: &str, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(value)
}
