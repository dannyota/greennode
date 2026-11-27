use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::Client;
use super::quota::{QuotaTimestamp, integer};
use super::response_times::{safe_time, serialize_time};
use super::wire::{Family, Read, check_path_id, decode, failure};
use crate::model::wire::WireField;
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Error, ErrorClass, Failure, RequestContext};

const UPTIME: Read = Read {
    id: "monitor.GetCheckAlarm",
    route: "vmonitor-uptime-manager/v1/uptimes/{}",
    limit: 1 << 20,
    family: Family::CheckAlarm,
};
const DEFINITION: Read = Read {
    route: "vmonitor-api/api/v1/alarms/synthetic/metrics/mona/{}",
    ..UPTIME
};

/// Selects the check to read its current alarm state.
#[derive(Clone)]
pub struct GetCheckAlarmInput {
    /// Check identifier.
    pub check_id: String,
}

crate::error::debug_record!(GetCheckAlarmInput { check_id });

impl GetCheckAlarmInput {
    /// Selects a check by its identifier.
    pub fn new(check_id: impl Into<String>) -> Self {
        Self {
            check_id: check_id.into(),
        }
    }
}

/// Current check alarm and location states.
#[derive(Clone, Serialize)]
pub struct GetCheckAlarmOutput {
    /// The current alarm definition and its location states.
    #[serde(rename = "Alarm")]
    pub alarm: CheckAlarm,
}

crate::wire::product::output::public_fields!(GetCheckAlarmOutput { alarm });

/// Current state of the check's alarm definition.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CheckAlarm {
    /// Alarm definition identifier.
    #[serde(rename = "ID")]
    pub id: String,
    /// Provider alarm status.
    pub status: String,
    /// Timestamp when the definition status changed.
    #[serde(serialize_with = "serialize_time")]
    pub status_updated_at: OffsetDateTime,
    /// Location states in provider order.
    pub locations: Vec<CheckLocationAlarm>,
}

crate::wire::product::output::public_fields!(CheckAlarm {
    id,
    status,
    status_updated_at,
    locations
});

/// State of one probe location's alarm.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CheckLocationAlarm {
    /// Identifier of the location alarm.
    #[serde(rename = "AlarmID")]
    pub alarm_id: String,
    /// Probe location identifier.
    #[serde(rename = "LocationID")]
    pub location_id: String,
    /// Display name of the probe location.
    pub location_name: String,
    /// Provider location alarm state.
    pub state: String,
    /// Timestamp when the location state changed.
    #[serde(serialize_with = "serialize_time")]
    pub state_updated_at: OffsetDateTime,
    /// Last update timestamp.
    #[serde(serialize_with = "serialize_time")]
    pub updated_at: OffsetDateTime,
    /// Creation timestamp.
    #[serde(serialize_with = "serialize_time")]
    pub created_at: OffsetDateTime,
}

crate::wire::product::output::public_fields!(CheckLocationAlarm {
    alarm_id,
    location_id,
    location_name,
    state,
    state_updated_at,
    updated_at,
    created_at
});

#[derive(Deserialize)]
struct Snapshot {
    id: String,
    history_alarm: String,
    options: CheckOptions,
}

#[derive(Deserialize)]
struct CheckOptions {
    test_frequency: Box<RawValue>,
}

#[derive(Deserialize)]
struct Envelope {
    data: Definition,
}

#[derive(Deserialize)]
struct Definition {
    id: String,
    status: Status,
    alarms: Vec<Location>,
}

#[derive(Deserialize)]
struct Status {
    status: String,
    status_updated_at: String,
}

#[derive(Deserialize)]
struct Location {
    id: String,
    metrics: Vec<Metric>,
    state: String,
    state_updated_timestamp: String,
    updated_timestamp: String,
    created_timestamp: String,
}

#[derive(Deserialize)]
struct Metric {
    dimensions: LocationName,
}

#[derive(Deserialize, PartialEq, Eq)]
struct LocationName {
    location_id: String,
    location_name: String,
}

macro_rules! object {
    ($($name:literal $(=> $shape:expr)?),+ $(,)?) => {
        Shape::Object(&[$(ShapeField {name: $name, shape: object!(@shape $($shape)?) }),+])
    };
    (@shape $shape:expr) => { $shape };
    (@shape) => { Shape::Any };
}

const SNAPSHOT: Shape<'static> =
    object!("id", "history_alarm", "options" => object!("test_frequency"));
const STATUS: Shape<'static> = object!("status", "status_updated_at");
const METRIC: Shape<'static> = object!("dimensions" => object!("location_id", "location_name"));
const LOCATION: Shape<'static> = object!("id", "metrics" => Shape::Array(&METRIC), "state", "state_updated_timestamp", "updated_timestamp", "created_timestamp");
const ENVELOPE: Shape<'static> =
    object!("data" => object!("id", "status" => STATUS, "alarms" => Shape::Array(&LOCATION)));

impl Client {
    /// Reads the alarm definition referenced by a check and projects its location states.
    pub async fn get_check_alarm(
        &self,
        ctx: &RequestContext,
        input: GetCheckAlarmInput,
    ) -> Result<GetCheckAlarmOutput, Failure<GetCheckAlarmOutput>> {
        crate::wire::product::exit::finish!(async {
            check_path_id(UPTIME.id, "CheckID", &input.check_id)?;
            let mut evidence = Evidence::new();
            let raw = self
                .read(ctx, &UPTIME, &[&input.check_id], &[], &mut evidence)
                .await
                .map_err(failure)?;
            let snapshot: Snapshot =
                decode(&raw, &SNAPSHOT, &UPTIME, &evidence).map_err(failure)?;
            if snapshot.id != input.check_id
                || evidence.id_reflects(&snapshot.id)
                || !integer(&snapshot.options.test_frequency).is_some_and(|value| value > 0)
            {
                return Err(failure(UPTIME.invalid(&evidence)));
            }
            let reference = snapshot.history_alarm;
            if reference.is_empty() {
                return Err(failure(Error::new(ErrorClass::NotFound).with_cause(
                    UPTIME.error(
                        None,
                        "NotFound",
                        "check has no alarm reference",
                        false,
                        &evidence,
                    ),
                )));
            }
            if !safe_id(&reference, &evidence) {
                return Err(failure(UPTIME.invalid(&evidence)));
            }
            let raw = self
                .read(ctx, &DEFINITION, &[&reference], &[], &mut evidence)
                .await
                .map_err(failure)?;
            let envelope: Envelope =
                decode(&raw, &ENVELOPE, &DEFINITION, &evidence).map_err(failure)?;
            if !safe_id(&reference, &evidence) || evidence.id_reflects(&snapshot.id) {
                return Err(failure(DEFINITION.invalid(&evidence)));
            }
            let alarm = project_alarm(envelope.data, &evidence).map_err(failure)?;
            Ok(GetCheckAlarmOutput { alarm })
        })
    }
}

fn project_alarm(definition: Definition, evidence: &Evidence) -> Result<CheckAlarm, Error> {
    let invalid = || DEFINITION.invalid(evidence);
    let mut status = definition.status.status;
    if !safe_id(&definition.id, evidence) || status.is_empty() || !status.redact(evidence) {
        return Err(invalid());
    }
    let status_updated_at =
        timestamp(&definition.status.status_updated_at, evidence).ok_or_else(invalid)?;
    if definition.alarms.len() > 64 {
        return Err(DEFINITION.error(
            Some(200),
            "FanOutLimitExceeded",
            "check alarm response rejected; body withheld",
            false,
            evidence,
        ));
    }
    let mut locations = Vec::with_capacity(definition.alarms.len());
    let mut location_ids = HashSet::new();
    let mut alarm_ids = HashSet::new();
    for wire in definition.alarms {
        let location = project_location(wire, evidence).ok_or_else(invalid)?;
        if !location_ids.insert(location.location_id.clone())
            || !alarm_ids.insert(location.alarm_id.clone())
        {
            return Err(invalid());
        }
        locations.push(location);
    }
    Ok(CheckAlarm {
        id: definition.id,
        status,
        status_updated_at,
        locations,
    })
}

fn project_location(wire: Location, evidence: &Evidence) -> Option<CheckLocationAlarm> {
    let mut metrics = wire.metrics.into_iter();
    let LocationName {
        location_id,
        mut location_name,
    } = metrics.next()?.dimensions;
    let mut state = wire.state;
    if !safe_id(&wire.id, evidence)
        || !safe_id(&location_id, evidence)
        || location_name.is_empty()
        || state.is_empty()
        || metrics.any(|metric| {
            metric.dimensions.location_id != location_id
                || metric.dimensions.location_name != location_name
        })
        || !location_name.redact(evidence)
        || !state.redact(evidence)
    {
        return None;
    }
    Some(CheckLocationAlarm {
        alarm_id: wire.id,
        location_id,
        location_name,
        state,
        state_updated_at: timestamp(&wire.state_updated_timestamp, evidence)?,
        updated_at: timestamp(&wire.updated_timestamp, evidence)?,
        created_at: timestamp(&wire.created_timestamp, evidence)?,
    })
}

fn timestamp(text: &str, evidence: &Evidence) -> Option<OffsetDateTime> {
    let value = OffsetDateTime::parse(text, &Rfc3339).ok()?;
    (value != QuotaTimestamp::default().0
        && !evidence.text_reflects(text)
        && safe_time(value, evidence))
    .then_some(value)
}

fn safe_id(id: &str, evidence: &Evidence) -> bool {
    PathIds::Strict.allows(id) && !evidence.id_reflects(id)
}
