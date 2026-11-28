use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{Date, Month, OffsetDateTime};

use super::models::{Field, Timestamp, nullable, redact_text};
use super::wire::{Call, convert, empty_data, read_failure};
use super::{Client, invalid};
use crate::{Failure, List, RequestContext};

const TRAFFIC: Call = Call::post("cdn.GetTraffic", "v1/analytic/traffic-consuming");
const REQUEST_RATE: Call = Call::post("cdn.GetRequestRate", "v1/analytic/cdn-requestsps");
const CACHE_STATUS: Call = Call::post("cdn.GetCacheStatus", "v1/analytic/cache-status");
const HTTP_CODES: Call = Call::post("cdn.GetHTTPCodes", "v1/analytic/cdn-http-codes");
const REPORT: Call = Call::post("cdn.GetTrafficReport", "v1/analytic/traffic-report");
const PERIODS: &[&str] = &[
    "30m", "1h", "3h", "6h", "12h", "24h", "3d", "7d", "14d", "30d", "90d", "180d", "360d",
];

/// CDN analytics scope and time range.
#[derive(Clone)]
pub struct Range {
    /// Generated CDN domains.
    pub cdn_domains: Vec<String>,
    /// Relative period such as `24h`.
    pub period: String,
    /// First calendar date in `YYYY-MM-DD` format.
    pub from: String,
    /// Last calendar date in `YYYY-MM-DD` format.
    pub to: String,
}

crate::error::debug_record!(Range {
    cdn_domains,
    period,
    from,
    to
});

impl Range {
    /// Creates a range for the selected CDN domains.
    pub fn new(cdn_domains: Vec<String>) -> Self {
        Self {
            cdn_domains,
            period: String::new(),
            from: String::new(),
            to: String::new(),
        }
    }

    fn body(&self) -> Option<Value> {
        if self.cdn_domains.is_empty()
            || self
                .cdn_domains
                .iter()
                .any(|domain| domain.trim().is_empty())
        {
            return None;
        }
        if !self.period.is_empty() {
            return (self.from.is_empty()
                && self.to.is_empty()
                && PERIODS.contains(&self.period.as_str()))
            .then(|| json!({"domains": self.cdn_domains, "period": self.period}));
        }
        let from = date(&self.from)?;
        let to = date(&self.to)?;
        (from <= to).then(|| {
            json!({
                "domains": self.cdn_domains,
                "fromTime": wire_date(from),
                "toTime": wire_date(to),
            })
        })
    }
}

/// Input for a dated CDN traffic report.
#[derive(Clone)]
pub struct GetTrafficReportInput {
    /// Generated CDN domains.
    pub cdn_domains: Vec<String>,
    /// First calendar date in `YYYY-MM-DD` format.
    pub from: String,
    /// Last calendar date in `YYYY-MM-DD` format.
    pub to: String,
}

crate::error::debug_record!(GetTrafficReportInput {
    cdn_domains,
    from,
    to
});

impl GetTrafficReportInput {
    /// Selects the domains and calendar dates for the report.
    pub fn new(cdn_domains: Vec<String>, from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            cdn_domains,
            from: from.into(),
            to: to.into(),
        }
    }
}

/// One sample in a CDN traffic or request rate series.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CacheSample {
    /// The UTC timestamp for the sample.
    pub time: Field<Timestamp>,
    /// The cached count or amount.
    pub cached: Field<f64>,
    /// The uncached count or amount.
    pub uncached: Field<f64>,
}

crate::wire::product::output::public_fields!(CacheSample {
    time,
    cached,
    uncached
});

/// One sample in a traffic report.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Sample {
    /// The UTC timestamp for the sample.
    pub time: Field<Timestamp>,
    /// The measured amount.
    pub value: Field<f64>,
}

crate::wire::product::output::public_fields!(Sample { time, value });

/// Traffic samples for a CDN domain and traffic group.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct DomainTraffic {
    /// The customer domain.
    pub domain_name: String,
    /// The generated CDN domain.
    #[serde(rename = "CDNDomain")]
    pub cdn_domain: String,
    /// The traffic category.
    pub traffic_type: String,
    /// The traffic group.
    pub group_type: String,
    /// The samples in ascending timestamp order.
    pub points: Vec<Sample>,
}

crate::wire::product::output::public_fields!(DomainTraffic {
    domain_name,
    cdn_domain,
    traffic_type,
    group_type,
    points
});

/// The traffic report grouped by CDN domain and traffic type.
pub type GetTrafficReportOutput = List<DomainTraffic>;

#[derive(Deserialize, Default)]
#[serde(default)]
struct CacheWire {
    #[serde(deserialize_with = "nullable")]
    cached: f64,
    #[serde(deserialize_with = "nullable")]
    uncached: f64,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct ReportWire {
    #[serde(deserialize_with = "nullable")]
    data: BTreeMap<String, f64>,
    #[serde(deserialize_with = "nullable")]
    domain_name: String,
    #[serde(deserialize_with = "nullable")]
    cdn_domain: String,
    #[serde(deserialize_with = "nullable")]
    traffic_type: String,
    #[serde(deserialize_with = "nullable")]
    group_type: String,
}

/// Input for `get_traffic`.
pub type GetTrafficInput = Range;

/// Output from `get_traffic`.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct GetTrafficOutput {
    /// Samples in ascending timestamp order.
    pub points: Vec<CacheSample>,
}

crate::wire::product::output::public_fields!(GetTrafficOutput { points });

impl Client {
    /// Reads the CDN traffic series.
    pub async fn get_traffic(
        &self,
        ctx: &RequestContext,
        input: GetTrafficInput,
    ) -> Result<GetTrafficOutput, Failure<GetTrafficOutput>> {
        crate::wire::product::exit::finish!(async {
            let points = self.series(ctx, &TRAFFIC, input).await.map_err(convert)?;
            Ok(GetTrafficOutput { points })
        })
    }
}

/// Input for `get_request_rate`.
pub type GetRequestRateInput = Range;

/// Output from `get_request_rate`.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct GetRequestRateOutput {
    /// Samples in ascending timestamp order.
    pub points: Vec<CacheSample>,
}

crate::wire::product::output::public_fields!(GetRequestRateOutput { points });

impl Client {
    /// Reads the CDN request rate series.
    pub async fn get_request_rate(
        &self,
        ctx: &RequestContext,
        input: GetRequestRateInput,
    ) -> Result<GetRequestRateOutput, Failure<GetRequestRateOutput>> {
        crate::wire::product::exit::finish!(async {
            let points = self
                .series(ctx, &REQUEST_RATE, input)
                .await
                .map_err(convert)?;
            Ok(GetRequestRateOutput { points })
        })
    }
}

/// Input for `get_cache_status`.
pub type GetCacheStatusInput = Range;

/// Output from `get_cache_status`.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct GetCacheStatusOutput {
    /// Counts keyed by the server’s category.
    pub counts: Option<BTreeMap<String, Field<f64>>>,
}

crate::wire::product::output::public_fields!(GetCacheStatusOutput { counts });

impl Client {
    /// Reads request counts by cache status.
    pub async fn get_cache_status(
        &self,
        ctx: &RequestContext,
        input: GetCacheStatusInput,
    ) -> Result<GetCacheStatusOutput, Failure<GetCacheStatusOutput>> {
        crate::wire::product::exit::finish!(async {
            let counts = self
                .counts(ctx, &CACHE_STATUS, input)
                .await
                .map_err(convert)?;
            Ok(GetCacheStatusOutput { counts })
        })
    }
}

/// Input for `get_http_codes`.
pub type GetHttpCodesInput = Range;

/// Output from `get_http_codes`.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct GetHttpCodesOutput {
    /// Counts keyed by the server’s category.
    pub counts: Option<BTreeMap<String, Field<f64>>>,
}

crate::wire::product::output::public_fields!(GetHttpCodesOutput { counts });

impl Client {
    /// Reads request counts by HTTP code.
    pub async fn get_http_codes(
        &self,
        ctx: &RequestContext,
        input: GetHttpCodesInput,
    ) -> Result<GetHttpCodesOutput, Failure<GetHttpCodesOutput>> {
        crate::wire::product::exit::finish!(async {
            let counts = self
                .counts(ctx, &HTTP_CODES, input)
                .await
                .map_err(convert)?;
            Ok(GetHttpCodesOutput { counts })
        })
    }
}

impl Client {
    /// Reads traffic by CDN domain and traffic group for the requested dates.
    pub async fn get_traffic_report(
        &self,
        ctx: &RequestContext,
        input: GetTrafficReportInput,
    ) -> Result<GetTrafficReportOutput, Failure<GetTrafficReportOutput>> {
        crate::wire::product::exit::finish!(async {
            let range = Range {
                cdn_domains: input.cdn_domains,
                from: input.from,
                to: input.to,
                period: String::new(),
            };
            let body = range.body().ok_or_else(invalid)?;
            let answer = self
                .call(ctx, &REPORT, &[], body.to_string().into())
                .await
                .map_err(convert)?;
            if empty_data(answer.data.as_deref()) {
                return Ok(List { items: Vec::new() });
            }
            let malformed = || read_failure(REPORT.unexpected(200));
            let text = answer.data.as_deref().ok_or_else(malformed)?.get();
            let rows: Vec<ReportWire> = crate::wire::product::lossy::from_slice(text.as_bytes())
                .map_err(|_| malformed())?;
            let evidence = &answer.evidence;
            let mut items = Vec::with_capacity(rows.len());
            for mut row in rows {
                let series = sorted(row.data).ok_or_else(malformed)?;
                let points = series
                    .into_iter()
                    .map(|(time, value)| {
                        let mut point = Sample {
                            time: Field::Value(time),
                            value: Field::Value(value),
                        };
                        point.time.redact(evidence);
                        point.value.redact(evidence);
                        point
                    })
                    .collect();
                for value in [
                    &mut row.domain_name,
                    &mut row.cdn_domain,
                    &mut row.traffic_type,
                    &mut row.group_type,
                ] {
                    redact_text(value, evidence);
                }
                items.push(DomainTraffic {
                    domain_name: row.domain_name,
                    cdn_domain: row.cdn_domain,
                    traffic_type: row.traffic_type,
                    group_type: row.group_type,
                    points,
                });
            }
            Ok(List { items })
        })
    }

    async fn series(
        &self,
        ctx: &RequestContext,
        call: &Call,
        input: Range,
    ) -> Result<Vec<CacheSample>, Failure<()>> {
        let body = input.body().ok_or_else(invalid)?;
        let answer = self.call(ctx, call, &[], body.to_string().into()).await?;
        if empty_data(answer.data.as_deref()) {
            return Ok(Vec::new());
        }
        let malformed = || read_failure(call.unexpected(200));
        let text = answer.data.as_deref().ok_or_else(malformed)?.get();
        let rows: BTreeMap<String, CacheWire> =
            crate::wire::product::lossy::from_slice(text.as_bytes()).map_err(|_| malformed())?;
        let rows = sorted(rows).ok_or_else(malformed)?;
        Ok(rows
            .into_iter()
            .map(|(time, row)| {
                let mut point = CacheSample {
                    time: Field::Value(time),
                    cached: Field::Value(row.cached),
                    uncached: Field::Value(row.uncached),
                };
                point.time.redact(&answer.evidence);
                point.cached.redact(&answer.evidence);
                point.uncached.redact(&answer.evidence);
                point
            })
            .collect())
    }

    async fn counts(
        &self,
        ctx: &RequestContext,
        call: &Call,
        input: Range,
    ) -> Result<Option<BTreeMap<String, Field<f64>>>, Failure<()>> {
        let body = input.body().ok_or_else(invalid)?;
        let answer = self.call(ctx, call, &[], body.to_string().into()).await?;
        let malformed = || read_failure(call.unexpected(200));
        let text = answer.data.as_deref().ok_or_else(malformed)?.get();
        let decoded: String =
            crate::wire::product::lossy::from_slice(text.as_bytes()).map_err(|_| malformed())?;
        if decoded.trim() == "[]" {
            return Ok(Some(BTreeMap::new()));
        }
        let mut counts: Option<BTreeMap<String, Field<f64>>> =
            crate::wire::product::lossy::from_slice(decoded.as_bytes()).map_err(|_| malformed())?;
        if let Some(counts) = &mut counts {
            counts.retain(|key, value| {
                value.redact(&answer.evidence);
                !answer.evidence.text_reflects(key)
            });
        }
        Ok(counts)
    }
}

fn sorted<T>(map: BTreeMap<String, T>) -> Option<Vec<(Timestamp, T)>> {
    let mut values = Vec::with_capacity(map.len());
    for (key, value) in map {
        let ms = key.parse::<i64>().ok()?;
        let time = OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).ok()?;
        values.push((Timestamp(time), value));
    }
    values.sort_by_key(|(time, _)| time.0);
    Some(values)
}

fn date(text: &str) -> Option<Date> {
    let [year, month, day] = text.split('-').collect::<Vec<_>>().try_into().ok()?;
    if [(year, 4), (month, 2), (day, 2)]
        .iter()
        .any(|(part, len)| part.len() != *len || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    Date::from_calendar_date(
        year.parse().ok()?,
        Month::try_from(month.parse::<u8>().ok()?).ok()?,
        day.parse().ok()?,
    )
    .ok()
}

fn wire_date(date: Date) -> String {
    format!(
        "{:02}/{:02}/{:04}",
        date.day(),
        u8::from(date.month()),
        date.year()
    )
}
