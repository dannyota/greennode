mod money;
mod wire;

use http::StatusCode;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::value::RawValue;

use super::envelope::invalid_response;
use super::model::null_default;
use super::{Client, read_operation};
use crate::error::SafeText;
use crate::wire::product::operation;
use crate::{ApiError, Error, ErrorClass, Failure, PagedList, RequestContext, WriteDispatch};

use money::decode_money;
use wire::{BalancesResponse, OverviewData, ResourcesData, SeriesItem};

const MAX_RESOURCE_SIZE: i64 = 200;

const OVERVIEW: &str = "billing.GetCostOverview";
const RESOURCES: &str = "billing.ListCostResources";
const BALANCES: &str = "billing.GetBalances";

/// Date range and optional filters for the cost overview.
#[derive(Clone, Debug)]
pub struct GetCostOverviewInput {
    /// First day of the range in YYYY-MM-DD form.
    pub start_date: String,
    /// Last day of the range in YYYY-MM-DD form.
    pub end_date: String,
    /// Optional product filter.
    pub product: String,
    /// Optional resource type filter.
    pub resource_type: String,
    /// Optional resource ID filter.
    pub resource_id: String,
    /// Optional search term, sent as `q`.
    pub query: String,
    /// Grouping sent to the API. An empty value selects `product`.
    pub group_by: String,
    /// Interval sent to the API. An empty value selects `daily`.
    pub interval: String,
}

impl GetCostOverviewInput {
    /// Sets the two required date fields with default grouping and interval.
    pub fn new(start_date: impl Into<String>, end_date: impl Into<String>) -> Self {
        Self {
            start_date: start_date.into(),
            end_date: end_date.into(),
            product: String::new(),
            resource_type: String::new(),
            resource_id: String::new(),
            query: String::new(),
            group_by: String::new(),
            interval: String::new(),
        }
    }
}

/// Headline costs and series returned for a date range.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct GetCostOverviewOutput {
    /// Current and prior costs, forecast, and breakdown.
    pub summary: CostSummary,
    /// Legacy daily points. An explicitly empty wire series stays empty.
    #[serde(serialize_with = "serialize_series")]
    pub series: Option<Vec<CostSeries>>,
    /// Grouped series in API order, when present.
    #[serde(serialize_with = "serialize_series")]
    pub grouped_series: Option<Vec<CostSeriesGroup>>,
    /// Interval reported by the API.
    pub interval: String,
    /// Start date reported by the API.
    pub start_date: String,
    /// End date reported by the API.
    pub end_date: String,
    /// Grouping reported by the API.
    pub group_by: String,
}

/// Date range, filters, ordering, and pagination for resource costs.
#[derive(Clone, Debug)]
pub struct ListCostResourcesInput {
    /// First day of the range in YYYY-MM-DD form.
    pub start_date: String,
    /// Last day of the range in YYYY-MM-DD form.
    pub end_date: String,
    /// Optional product filter.
    pub product: String,
    /// Optional resource type filter.
    pub resource_type: String,
    /// Optional resource ID filter.
    pub resource_id: String,
    /// Optional search term, sent as `q`.
    pub query: String,
    /// One-based page. Nonpositive values select page one.
    pub page: i64,
    /// Page size. Values outside 1 to 200 select 200.
    pub size: i64,
    /// Optional sort field.
    pub sort: String,
    /// Optional sort direction.
    pub order: String,
}

impl ListCostResourcesInput {
    /// Sets the two required date fields and uses the first page by default.
    pub fn new(start_date: impl Into<String>, end_date: impl Into<String>) -> Self {
        Self {
            start_date: start_date.into(),
            end_date: end_date.into(),
            product: String::new(),
            resource_type: String::new(),
            resource_id: String::new(),
            query: String::new(),
            page: 0,
            size: 0,
            sort: String::new(),
            order: String::new(),
        }
    }
}

/// One page of resource costs and the range's overall summary.
#[derive(Clone, Debug, PartialEq)]
pub struct ListCostResourcesOutput {
    /// Resource costs and pagination metadata returned by the API.
    pub resources: PagedList<CostResource>,
    /// Totals across the matching resources.
    pub summary: CostResourceSummary,
}

impl Serialize for ListCostResourcesOutput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut output = serializer.serialize_struct("ListCostResourcesOutput", 6)?;
        output.serialize_field("Items", &self.resources.items)?;
        output.serialize_field("Page", &self.resources.page)?;
        output.serialize_field("PageSize", &self.resources.page_size)?;
        output.serialize_field("TotalPage", &self.resources.total_page)?;
        output.serialize_field("TotalItem", &self.resources.total_item)?;
        output.serialize_field("Summary", &self.summary)?;
        output.end()
    }
}

/// Selects balances for the authenticated account.
#[derive(Clone, Debug, Default)]
pub struct GetBalancesInput {}

/// Cash and pay-on-credit balances for the authenticated account.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetBalancesOutput {
    /// Balance amounts by kind.
    #[serde(rename = "Balances")]
    pub balances: Balances,
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Reads cost totals and series for the given dates and filters.
    pub async fn get_cost_overview(
        &self,
        ctx: &RequestContext,
        input: GetCostOverviewInput,
    ) -> Result<GetCostOverviewOutput, Failure<GetCostOverviewOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut query = cost_query(
                &input.start_date,
                &input.end_date,
                [
                    ("product", &input.product),
                    ("resourceType", &input.resource_type),
                    ("resourceId", &input.resource_id),
                    ("q", &input.query),
                ],
            )?;
            query.extend([
                ("groupBy", fallback(&input.group_by, "product")),
                ("interval", fallback(&input.interval, "daily")),
            ]);
            let data: OverviewData = self
                .read(
                    ctx,
                    &read_operation(OVERVIEW, "gateway/api/v2/cost-explorer/overview"),
                    &[],
                    &query,
                )
                .await?;
            let mut series = data
                .series
                .as_ref()
                .filter(|items| items.is_empty())
                .map(|_| Vec::new());
            let mut grouped_series = None;
            for item in data.series.into_iter().flatten() {
                match item {
                    SeriesItem::Flat(point) => series.get_or_insert_with(Vec::new).push(point),
                    SeriesItem::Group(group) => {
                        grouped_series.get_or_insert_with(Vec::new).push(group)
                    }
                }
            }
            Ok(GetCostOverviewOutput {
                summary: data.summary,
                series,
                grouped_series,
                interval: data.interval,
                start_date: data.start_date,
                end_date: data.end_date,
                group_by: data.group_by,
            })
        })
    }

    /// Reads one page of resource costs. Page numbers start at one.
    pub async fn list_cost_resources(
        &self,
        ctx: &RequestContext,
        input: ListCostResourcesInput,
    ) -> Result<ListCostResourcesOutput, Failure<ListCostResourcesOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut query = cost_query(
                &input.start_date,
                &input.end_date,
                [
                    ("product", &input.product),
                    ("resourceType", &input.resource_type),
                    ("resourceId", &input.resource_id),
                    ("q", &input.query),
                    ("sort", &input.sort),
                    ("order", &input.order),
                ],
            )?;
            let page = (input.page.max(1) - 1).to_string();
            let size = if (1..=MAX_RESOURCE_SIZE).contains(&input.size) {
                input.size
            } else {
                MAX_RESOURCE_SIZE
            }
            .to_string();
            query.extend([("page", page.as_str()), ("size", size.as_str())]);

            let operation = read_operation(RESOURCES, "gateway/api/v2/cost-explorer/resources");
            let data: ResourcesData = self.read(ctx, &operation, &[], &query).await?;
            let page = data
                .pagination
                .page
                .checked_add(1)
                .filter(|page| *page > 0)
                .ok_or_else(|| {
                    read_failure(
                        Error::from_api(ApiError::without_fallback(
                            RESOURCES,
                            Some(StatusCode::OK.as_u16()),
                            SafeText::new("InvalidResponse", &[]),
                            SafeText::new("invalid cost resource page; body withheld", &[]),
                            false,
                        ))
                        .with_outer_detail(StatusCode::OK),
                    )
                })?;
            Ok(ListCostResourcesOutput {
                resources: PagedList {
                    items: data.items,
                    page,
                    page_size: data.pagination.size,
                    total_page: data.pagination.total_pages,
                    total_item: data.pagination.total,
                },
                summary: data.summary,
            })
        })
    }

    /// Reads the authenticated account's available and held balances.
    pub async fn get_balances(
        &self,
        ctx: &RequestContext,
        _: GetBalancesInput,
    ) -> Result<GetBalancesOutput, Failure<GetBalancesOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation(BALANCES, "navbar/balances/v1");
            let session = &self.session;
            let base = session
                .endpoints()
                .billing
                .as_ref()
                .ok_or_else(|| read_failure(Error::new(ErrorClass::InvalidConfig)))?;
            let mut jitter = || session.retry_jitter();
            let runtime = session
                .runtime(ctx, &mut jitter)
                .map_err(|error| read_failure(*error))?;
            let (_, BalancesResponse(balances)) = operation::execute(
                runtime,
                base,
                &operation,
                &[],
                &[],
                session.bearer_source(),
                bytes::Bytes::new(),
            )
            .await
            .map_err(|error| {
                let (error, _, dispatch) = error.into_parts();
                let error = match error.detail::<StatusCode>().copied() {
                    Some(status)
                        if status.is_success()
                            && error.class() == ErrorClass::Api
                            && error
                                .api_error()
                                .is_none_or(|api| api.code() != "InvalidResponse") =>
                    {
                        invalid_response(&operation, status, error.is_body_too_large())
                            .with_cause(error)
                    }
                    _ => error,
                };
                Failure::new(error, None, dispatch)
            })?;
            Ok(GetBalancesOutput { balances })
        })
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
fn cost_query<'a, O, const N: usize>(
    start: &'a str,
    end: &'a str,
    filters: [(&'static str, &'a str); N],
) -> Result<Vec<(&'static str, &'a str)>, Failure<O>> {
    if !valid_date(start) || !valid_date(end) {
        return Err(read_failure(Error::new(ErrorClass::InvalidInput)));
    }
    let mut query = vec![("startDate", start), ("endDate", end)];
    query.extend(filters.into_iter().filter(|(_, value)| !value.is_empty()));
    Ok(query)
}

fn valid_date(value: &str) -> bool {
    let Some((year, rest)) = value.split_once('-') else {
        return false;
    };
    let Some((month, day)) = rest.split_once('-') else {
        return false;
    };
    if [(year, 4), (month, 2), (day, 2)]
        .into_iter()
        .any(|(part, length)| {
            part.len() != length || !part.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return false;
    }
    let date = || {
        time::Date::from_calendar_date(
            year.parse().ok()?,
            time::Month::try_from(month.parse::<u8>().ok()?).ok()?,
            day.parse().ok()?,
        )
        .ok()
    };
    date().is_some()
}

fn fallback<'a>(value: &'a str, default: &'a str) -> &'a str {
    if value.is_empty() { default } else { value }
}

fn read_failure<O>(error: Error) -> Failure<O> {
    Failure::new(error, None, WriteDispatch::NotAWrite)
}

/// One entry in the cost breakdown.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CostBreakdown {
    /// Group key reported by the API.
    pub key: String,
    /// Cost in VND.
    pub cost: f64,
    /// Share of the total cost as a percentage.
    pub percent: f64,
}

impl<'de> Deserialize<'de> for CostBreakdown {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Fields<'a> {
            #[serde(deserialize_with = "null_default")]
            key: String,
            #[serde(borrow, deserialize_with = "present")]
            value: Option<&'a RawValue>,
            #[serde(borrow, deserialize_with = "present")]
            cost: Option<&'a RawValue>,
            #[serde(deserialize_with = "null_default")]
            percent: f64,
        }

        let fields = Option::<Fields<'_>>::deserialize(deserializer)?.unwrap_or_default();
        let cost = match fields.value.or(fields.cost) {
            Some(raw) => serde_json::from_str::<Option<f64>>(raw.get())
                .map_err(|_| de::Error::custom("invalid breakdown cost"))?
                .unwrap_or_default(),
            None => 0.0,
        };
        Ok(Self {
            key: fields.key,
            cost,
            percent: fields.percent,
        })
    }
}

/// Headline cost totals for the selected dates.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
pub struct CostSummary {
    /// Total cost in VND for the requested period.
    pub current_cost: f64,
    /// Total cost in VND in the previous period.
    #[serde(default, deserialize_with = "null_default")]
    pub last_period_cost: f64,
    /// Percentage change from the previous period.
    #[serde(default, deserialize_with = "null_default")]
    pub change_percent: f64,
    /// Forecasted cost in VND.
    #[serde(default, deserialize_with = "null_default")]
    pub forecast_cost: f64,
    /// Number of active resources.
    #[serde(default, deserialize_with = "null_default")]
    pub active_count: i64,
    /// Breakdown of costs by key.
    #[serde(default, deserialize_with = "null_default")]
    pub breakdown: Vec<CostBreakdown>,
}

/// A legacy date and cost point.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
pub struct CostSeries {
    /// Date as reported by the API.
    pub date: String,
    /// Cost in VND for that date.
    pub cost: f64,
}

/// A group in the cost overview.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
pub struct CostSeriesGroup {
    /// Group key reported by the API.
    pub key: String,
    /// Display color reported by the API.
    #[serde(default, deserialize_with = "null_default")]
    pub color: String,
    /// Ordered points in the group.
    pub points: Vec<CostSeriesPoint>,
}

/// One bucket in a grouped cost series.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
pub struct CostSeriesPoint {
    /// Unparsed API bucket label.
    #[serde(rename(deserialize = "x"))]
    pub date: String,
    /// Cost in VND for the bucket.
    #[serde(rename(deserialize = "y"))]
    pub cost: f64,
}

/// Cost of a resource over the requested range.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
pub struct CostResource {
    /// Resource identifier.
    #[serde(
        default,
        rename(serialize = "ResourceID"),
        deserialize_with = "null_default"
    )]
    pub resource_id: String,
    /// Resource display name.
    #[serde(default, deserialize_with = "null_default")]
    pub resource_name: String,
    /// Resource type reported by the API.
    #[serde(default, deserialize_with = "null_default")]
    pub resource_type: String,
    /// Product that billed the resource.
    #[serde(default, deserialize_with = "null_default")]
    pub product: String,
    /// Resource's total cost in VND over the requested range.
    #[serde(rename(deserialize = "totalCost"))]
    pub cost: f64,
    /// Resource's average daily cost in VND.
    #[serde(
        rename(deserialize = "dailyAvg"),
        default,
        deserialize_with = "null_default"
    )]
    pub daily_average: f64,
    /// Resource's start date as reported by the API.
    #[serde(default, deserialize_with = "null_default")]
    pub start_date: String,
}

/// Aggregate costs across matching resources.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
pub struct CostResourceSummary {
    /// Total cost in VND for all matching resources.
    pub total_all: f64,
    /// Number of active resources.
    pub active_count: i64,
    /// Average cost in VND per matching resource.
    pub avg_per_resource: f64,
}

/// Cash and pay-on-credit balances. A missing balance kind is reported as `None`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
pub struct Balances {
    /// Cash balance.
    #[serde(deserialize_with = "decode_money")]
    pub cash: Option<f64>,
    /// Pay-on-credit balance.
    #[serde(rename(serialize = "POC"), deserialize_with = "decode_money")]
    pub poc: Option<f64>,
    /// Cash available for use.
    #[serde(deserialize_with = "decode_money")]
    pub cash_available: Option<f64>,
    /// Cash on hold.
    #[serde(deserialize_with = "decode_money")]
    pub cash_holding: Option<f64>,
    /// Pay-on-credit funds on hold.
    #[serde(rename(serialize = "POCHolding"), deserialize_with = "decode_money")]
    pub poc_holding: Option<f64>,
}

fn serialize_series<S: Serializer, T: Serialize>(
    series: &Option<Vec<T>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    series.as_deref().unwrap_or_default().serialize(serializer)
}

fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<&'de RawValue>, D::Error> {
    <&RawValue>::deserialize(deserializer).map(Some)
}
