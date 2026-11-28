use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, de};
use serde_json::value::RawValue;

use super::super::envelope::{BudgetData, Envelope, decode_header, invalid_response};
use super::super::model::{field, null_default, reflects_integer};
use super::super::response_validation::data_valid;
use super::{
    Balances, CostResource, CostResourceSummary, CostSeries, CostSeriesGroup, CostSummary,
};
use crate::wire::product::{
    lossy,
    operation::{self, Operation, Output},
    sender::{Response, SentCredentials},
};
use crate::wire::strict_json::{self, Shape, ShapeField};
use crate::{Error, output_number};

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OverviewData {
    pub(super) summary: CostSummary,
    pub(super) series: Option<Vec<SeriesItem>>,
    #[serde(default, deserialize_with = "null_default")]
    pub(super) interval: String,
    #[serde(default, deserialize_with = "null_default")]
    pub(super) start_date: String,
    #[serde(default, deserialize_with = "null_default")]
    pub(super) end_date: String,
    #[serde(default, deserialize_with = "null_default")]
    pub(super) group_by: String,
}

pub(super) enum SeriesItem {
    Group(CostSeriesGroup),
    Flat(CostSeries),
}

impl<'de> Deserialize<'de> for SeriesItem {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = <&RawValue>::deserialize(deserializer)?;
        let fields: BTreeMap<String, &RawValue> =
            serde_json::from_str(raw.get()).map_err(de::Error::custom)?;
        let grouped = fields.keys().any(|name| {
            ["key", "color", "points"]
                .iter()
                .any(|field| strict_json::field_matches(field, name))
        });
        const GROUP: Shape<'static> = Shape::Object(&[
            field("key"),
            field("color"),
            shape_field(
                "points",
                Shape::Array(&Shape::Object(&[field("x"), field("y")])),
            ),
        ]);
        const FLAT: Shape<'static> = Shape::Object(&[field("date"), field("cost")]);
        let shape = if grouped { &GROUP } else { &FLAT };
        strict_json::validate_canonical_shape(raw.get().as_bytes(), shape, raw.get().len())
            .map_err(|_| de::Error::custom("invalid cost series fields"))?;
        if grouped {
            serde_json::from_str(raw.get())
                .map(Self::Group)
                .map_err(de::Error::custom)
        } else {
            serde_json::from_str(raw.get())
                .map(Self::Flat)
                .map_err(de::Error::custom)
        }
    }
}

#[derive(Default, Deserialize)]
pub(super) struct ResourcesData {
    pub(super) items: Vec<CostResource>,
    pub(super) pagination: ResourcePage,
    pub(super) summary: CostResourceSummary,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ResourcePage {
    pub(super) page: i64,
    pub(super) size: i64,
    pub(super) total: i64,
    pub(super) total_pages: i64,
}

#[derive(Default, Deserialize)]
pub(super) struct BalancesResponse(pub(super) Balances);

impl Output for BalancesResponse {
    const SCHEMA: Shape<'static> = Balances::SCHEMA;

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        let invalid = || Box::new(invalid_response(operation, response.status, false));
        if operation.success.contains(&response.status) {
            let direct = response.decode_projection(|body| {
                decode_header(body, operation.cap)
                    .map(|header| header.is_direct())
                    .ok_or_else(invalid)
            })?;
            if direct {
                response.decode_projection(|body| {
                    if body.len() > operation.cap {
                        return Err(invalid());
                    }
                    lossy::with_normalized(body, |text| {
                        strict_json::validate_folded_canonical_shape(
                            text.as_bytes(),
                            &Self::SCHEMA,
                            text.len(),
                        )
                        .map_err(|_| invalid())?;
                        let raw = serde_json::from_str(text).map_err(|_| invalid())?;
                        if !data_valid(operation.id, Some(raw)) {
                            return Err(invalid());
                        }
                        Ok(())
                    })
                })?;
                if response.credentials.has_numeric_product_credential() {
                    return Err(invalid());
                }
                let result: Self =
                    operation::decode_response(operation, response).map_err(|_| invalid())?;
                if !result.0.valid(&response.credentials) {
                    return Err(invalid());
                }
                return Ok(result);
            }
        }
        Envelope::<Balances>::decode_response(operation, response)
            .map(|envelope| Self(envelope.data))
    }
}

const fn shape_field(name: &'static str, shape: Shape<'static>) -> ShapeField<'static> {
    ShapeField { name, shape }
}

impl BudgetData for OverviewData {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        shape_field(
            "summary",
            Shape::Object(&[
                field("currentCost"),
                field("lastPeriodCost"),
                field("changePercent"),
                field("forecastCost"),
                field("activeCount"),
                shape_field(
                    "breakdown",
                    Shape::Array(&Shape::Object(&[
                        field("key"),
                        field("value"),
                        field("cost"),
                        field("percent"),
                    ])),
                ),
            ]),
        ),
        shape_field("series", Shape::Array(&Shape::Any)),
        field("interval"),
        field("startDate"),
        field("endDate"),
        field("groupBy"),
    ]);

    fn valid(&self, credentials: &SentCredentials) -> bool {
        let summary = &self.summary;
        let totals = [
            summary.current_cost,
            summary.last_period_cost,
            summary.change_percent,
            summary.forecast_cost,
        ];
        !reflects_integer(summary.active_count, credentials)
            && !totals
                .into_iter()
                .chain(
                    summary
                        .breakdown
                        .iter()
                        .flat_map(|item| [item.cost, item.percent]),
                )
                .any(|value| output_number::reflects(value, credentials, true))
            && self.series.iter().flatten().all(|item| match item {
                SeriesItem::Flat(point) => !output_number::reflects(point.cost, credentials, true),
                SeriesItem::Group(group) => group
                    .points
                    .iter()
                    .all(|point| !output_number::reflects(point.cost, credentials, true)),
            })
    }
}

impl BudgetData for ResourcesData {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        shape_field(
            "items",
            Shape::Array(&Shape::Object(&[
                field("resourceId"),
                field("resourceName"),
                field("resourceType"),
                field("product"),
                field("totalCost"),
                field("dailyAvg"),
                field("startDate"),
            ])),
        ),
        shape_field(
            "pagination",
            Shape::Object(&[
                field("page"),
                field("size"),
                field("total"),
                field("totalPages"),
            ]),
        ),
        shape_field(
            "summary",
            Shape::Object(&[
                field("totalAll"),
                field("activeCount"),
                field("avgPerResource"),
            ]),
        ),
    ]);

    fn valid(&self, credentials: &SentCredentials) -> bool {
        ![
            self.pagination.page.checked_add(1),
            Some(self.pagination.size),
            Some(self.pagination.total),
            Some(self.pagination.total_pages),
            Some(self.summary.active_count),
        ]
        .into_iter()
        .flatten()
        .any(|value| reflects_integer(value, credentials))
            && ![self.summary.total_all, self.summary.avg_per_resource]
                .into_iter()
                .chain(
                    self.items
                        .iter()
                        .flat_map(|item| [item.cost, item.daily_average]),
                )
                .any(|value| output_number::reflects(value, credentials, true))
    }
}

impl BudgetData for Balances {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        field("cash"),
        field("poc"),
        field("cashAvailable"),
        field("cashHolding"),
        field("pocHolding"),
    ]);

    fn valid(&self, credentials: &SentCredentials) -> bool {
        ![
            self.cash,
            self.poc,
            self.cash_available,
            self.cash_holding,
            self.poc_holding,
        ]
        .into_iter()
        .flatten()
        .any(|value| output_number::reflects(value, credentials, true))
    }
}
