use std::collections::HashSet;

use serde::{Deserialize, Serialize, Serializer};
use time::{Duration, OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

use super::wire::{Family, Read, check_path_id, decode, failure};
use super::{Client, ResponseField};
use crate::model::wire::WireField;
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Error, ErrorClass, Failure, RequestContext};

const CHECK: Read = Read {
    id: "monitor.ListCheckResponseTimes",
    route: "vmonitor-uptime-manager/v1/uptimes/{}",
    limit: 1 << 20,
    family: Family::CheckAlarm,
};
const STATISTICS: Read = Read {
    route: "vmonitor-api/api/v1/statistics/synthetics",
    limit: 16 << 20,
    ..CHECK
};
const METRIC: &str = "syntt.http.response_time";

/// Check and optional RFC 3339 window for a timing read.
#[derive(Clone, Debug)]
pub struct ListCheckResponseTimesInput {
    /// Identifier of the HTTP check.
    pub check_id: String,
    /// Window start. Empty selects 24 hours before the end.
    pub start: String,
    /// Window end. Empty selects the current time.
    pub end: String,
}

impl ListCheckResponseTimesInput {
    /// Selects a check and the latest 24 hours.
    pub fn new(check_id: impl Into<String>) -> Self {
        Self {
            check_id: check_id.into(),
            start: String::new(),
            end: String::new(),
        }
    }
}

/// Response-time series and the effective UTC window.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ListCheckResponseTimesOutput {
    /// Series sorted by location identifier.
    pub items: Vec<CheckResponseTimeSeries>,
    /// Effective start rounded down to millisecond precision.
    #[serde(serialize_with = "serialize_time")]
    pub start: OffsetDateTime,
    /// Effective end rounded down to millisecond precision.
    #[serde(serialize_with = "serialize_time")]
    pub end: OffsetDateTime,
}

crate::wire::product::output::public_fields!(ListCheckResponseTimesOutput { items, start, end });

/// The response-time buckets for one probe location.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CheckResponseTimeSeries {
    /// Probe location identifier.
    #[serde(rename = "LocationID")]
    pub location_id: String,
    /// Buckets sorted by timestamp.
    pub points: Vec<CheckResponseTimePoint>,
}

crate::wire::product::output::public_fields!(CheckResponseTimeSeries {
    location_id,
    points
});

/// Average HTTP response time for one bucket.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CheckResponseTimePoint {
    /// UTC bucket timestamp.
    #[serde(serialize_with = "serialize_time")]
    pub timestamp: OffsetDateTime,
    /// Average in milliseconds.
    pub value_millis: ResponseField<f64>,
}

crate::wire::product::output::public_fields!(CheckResponseTimePoint {
    timestamp,
    value_millis
});

#[derive(Deserialize)]
struct Snapshot {
    id: String,
    subtype: String,
    history_alarm: String,
    locations: Vec<String>,
}

#[derive(Deserialize)]
struct Group {
    name: String,
    dimensions: Dimensions,
    statistics: Vec<Bucket>,
}

#[derive(Deserialize)]
struct Dimensions {
    location_id: String,
}

#[derive(Deserialize)]
struct Bucket {
    key: String,
    value: String,
}

const SNAPSHOT: Shape<'static> = Shape::Object(&[
    ShapeField {
        name: "id",
        shape: Shape::Any,
    },
    ShapeField {
        name: "subtype",
        shape: Shape::Any,
    },
    ShapeField {
        name: "history_alarm",
        shape: Shape::Any,
    },
    ShapeField {
        name: "locations",
        shape: Shape::Array(&Shape::Any),
    },
]);
const GROUP: Shape<'static> = Shape::Object(&[
    ShapeField {
        name: "name",
        shape: Shape::Any,
    },
    ShapeField {
        name: "dimensions",
        shape: Shape::Object(&[ShapeField {
            name: "location_id",
            shape: Shape::Any,
        }]),
    },
    ShapeField {
        name: "statistics",
        shape: Shape::Array(&Shape::Object(&[
            ShapeField {
                name: "key",
                shape: Shape::Any,
            },
            ShapeField {
                name: "value",
                shape: Shape::Any,
            },
        ])),
    },
]);

impl Client {
    /// Reads average response times for an HTTP check, without mutating it.
    pub async fn list_check_response_times(
        &self,
        ctx: &RequestContext,
        input: ListCheckResponseTimesInput,
    ) -> Result<ListCheckResponseTimesOutput, Failure<ListCheckResponseTimesOutput>> {
        crate::wire::product::exit::finish!(async {
            check_path_id(CHECK.id, "CheckID", &input.check_id)?;
            let mut evidence = Evidence::new();
            let Some(window) = Window::parse(&input, self.session.clock().utc_now()) else {
                return Err(failure(Error::new(ErrorClass::InvalidInput).with_cause(
                    CHECK.error(
                        None,
                        "InvalidInput",
                        "Start and End must form a nonempty RFC 3339 range at millisecond precision",
                        false,
                        &evidence,
                    ),
                )));
            };
            let raw = self
                .read(ctx, &CHECK, &[&input.check_id], &[], &mut evidence)
                .await
                .map_err(failure)?;
            let snapshot: Snapshot = decode(&raw, &SNAPSHOT, &CHECK, &evidence).map_err(failure)?;
            let locations = snapshot
                .locations
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>();
            if snapshot.id != input.check_id
                || snapshot.subtype.is_empty()
                || locations.len() != snapshot.locations.len()
                || locations
                    .iter()
                    .any(|location| !PathIds::Strict.allows(location))
                || (!snapshot.history_alarm.is_empty()
                    && !PathIds::Strict.allows(&snapshot.history_alarm))
                || snapshot.decision_reflects(&evidence)
            {
                return Err(failure(CHECK.invalid(&evidence)));
            }
            if snapshot.subtype != "HTTP" {
                return Err(failure(Error::new(ErrorClass::InvalidInput).with_cause(
                    CHECK.error(
                        None,
                        "InvalidInput",
                        "check subtype must be HTTP",
                        false,
                        &evidence,
                    ),
                )));
            }
            if snapshot.history_alarm.is_empty() {
                return Err(failure(Error::new(ErrorClass::NotFound).with_cause(
                    CHECK.error(
                        None,
                        "NotFound",
                        "check has no alarm reference",
                        false,
                        &evidence,
                    ),
                )));
            }
            let query = [
                ("alarm", snapshot.history_alarm.as_str()),
                ("dimensions", &format!("syntt_id:{}", input.check_id)),
                ("end_time", &milliseconds(window.end).to_string()),
                ("group_by", "location_id"),
                ("name", METRIC),
                ("start_time", &milliseconds(window.start).to_string()),
                ("statistics", "avg"),
            ];
            let raw = self
                .read(ctx, &STATISTICS, &[], &query, &mut evidence)
                .await
                .map_err(failure)?;
            if snapshot.decision_reflects(&evidence)
                || !safe_time(window.start, &evidence)
                || !safe_time(window.end, &evidence)
            {
                return Err(failure(CHECK.invalid(&evidence)));
            }
            check_limits(&raw, &evidence).map_err(failure)?;
            let groups =
                decode(&raw, &Shape::Array(&GROUP), &STATISTICS, &evidence).map_err(failure)?;
            let items = series(groups, locations, &window, &evidence).map_err(failure)?;
            Ok(ListCheckResponseTimesOutput {
                items,
                start: window.start,
                end: window.end,
            })
        })
    }
}

impl Snapshot {
    fn decision_reflects(&self, evidence: &Evidence) -> bool {
        evidence.text_reflects(&self.subtype)
            || [&self.id, &self.history_alarm]
                .into_iter()
                .chain(&self.locations)
                .any(|value| evidence.id_reflects(value))
    }
}

fn check_limits(raw: &serde_json::value::RawValue, evidence: &Evidence) -> Result<(), Error> {
    let Ok(groups) = serde_json::from_str::<Vec<&serde_json::value::RawValue>>(raw.get()) else {
        return Ok(());
    };
    let limit = || {
        CHECK.error(
            Some(200),
            "ResponseLimitExceeded",
            "check alarm response rejected; body withheld",
            false,
            evidence,
        )
    };
    if groups.len() > 64 {
        return Err(limit());
    }
    let mut count = 0usize;
    for group in groups {
        let Ok(fields) = serde_json::from_str::<
            std::collections::HashMap<&str, &serde_json::value::RawValue>,
        >(group.get()) else {
            return Ok(());
        };
        let Some(statistics) = fields.get("statistics") else {
            return Ok(());
        };
        let Ok(buckets) =
            serde_json::from_str::<Vec<&serde_json::value::RawValue>>(statistics.get())
        else {
            return Ok(());
        };
        count = count.saturating_add(buckets.len());
        if count > 100_000 {
            return Err(limit());
        }
    }
    Ok(())
}

struct Window {
    start: OffsetDateTime,
    end: OffsetDateTime,
}

impl Window {
    fn parse(input: &ListCheckResponseTimesInput, now: OffsetDateTime) -> Option<Self> {
        let end = if input.end.is_empty() {
            now
        } else {
            parse_time(&input.end)?
        };
        let start = if input.start.is_empty() {
            end.checked_sub(Duration::DAY)?
        } else {
            parse_time(&input.start)?
        };
        let start =
            OffsetDateTime::from_unix_timestamp_nanos(milliseconds(start) * 1_000_000).ok()?;
        let end = OffsetDateTime::from_unix_timestamp_nanos(milliseconds(end) * 1_000_000).ok()?;
        (start < end && end - start <= Duration::DAY && start.year() >= 1 && end.year() <= 9999)
            .then_some(Self { start, end })
    }
}

fn milliseconds(time: OffsetDateTime) -> i128 {
    time.unix_timestamp_nanos().div_euclid(1_000_000)
}

pub(super) fn serialize_time<S: Serializer>(
    value: &OffsetDateTime,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let text = value.format(&Rfc3339).map_err(serde::ser::Error::custom)?;
    serializer.serialize_str(&text)
}

pub(super) fn safe_time(time: OffsetDateTime, evidence: &Evidence) -> bool {
    time.format(&Rfc3339)
        .is_ok_and(|text| !evidence.text_reflects(&text))
}

fn parse_time(text: &str) -> Option<OffsetDateTime> {
    let seconds = text.get(17..19)?.parse::<u8>().ok()?;
    if seconds > 59
        || text.contains(',')
        || !text
            .as_bytes()
            .get(10)
            .is_some_and(|byte| matches!(byte, b'T' | b't'))
    {
        return None;
    }
    let time = OffsetDateTime::parse(text, &Rfc3339)
        .ok()?
        .checked_to_offset(UtcOffset::UTC)?;
    (1..=9999).contains(&time.year()).then_some(time)
}

fn series(
    groups: Vec<Group>,
    locations: HashSet<&str>,
    window: &Window,
    evidence: &Evidence,
) -> Result<Vec<CheckResponseTimeSeries>, Error> {
    let invalid = || CHECK.invalid(evidence);
    let mut seen = HashSet::new();
    let mut count = 0;
    let mut items = Vec::with_capacity(groups.len().min(64));
    for group in groups {
        count += group.statistics.len();
        if items.len() >= 64 || count > 100_000 {
            return Err(CHECK.error(
                Some(200),
                "ResponseLimitExceeded",
                "check alarm response rejected; body withheld",
                false,
                evidence,
            ));
        }
        let mut location_id = group.dimensions.location_id;
        if group.name != METRIC
            || !locations.contains(location_id.as_str())
            || !seen.insert(location_id.clone())
        {
            return Err(invalid());
        }
        let mut points = Vec::with_capacity(group.statistics.len());
        let mut timestamps = HashSet::new();
        let mut early = false;
        for bucket in group.statistics {
            let timestamp = parse_time(&bucket.key).ok_or_else(invalid)?;
            if timestamp < window.start {
                if early
                    || window.end - window.start != Duration::DAY
                    || window.start - timestamp > Duration::minutes(5)
                {
                    return Err(invalid());
                }
                early = true;
            }
            let number = decimal(&bucket.value).ok_or_else(invalid)?;
            if evidence.text_reflects(&bucket.value) {
                return Err(invalid());
            }
            let mut value_millis = ResponseField::Value(number);
            if timestamp > window.end
                || !timestamps.insert(timestamp)
                || evidence.text_reflects(&bucket.key)
                || !safe_time(timestamp, evidence)
                || !value_millis.redact(evidence)
            {
                return Err(invalid());
            }
            points.push(CheckResponseTimePoint {
                timestamp,
                value_millis,
            });
        }
        if !location_id.redact(evidence) {
            return Err(invalid());
        }
        points.sort_by_key(|point| point.timestamp);
        items.push(CheckResponseTimeSeries {
            location_id,
            points,
        });
    }
    items.sort_by(|left, right| left.location_id.cmp(&right.location_id));
    Ok(items)
}

fn decimal(text: &str) -> Option<f64> {
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    if !text.split_once('.').map_or_else(
        || digits(text),
        |(whole, fraction)| digits(whole) && digits(fraction),
    ) {
        return None;
    }
    text.parse::<f64>().ok().filter(|value| value.is_finite())
}
