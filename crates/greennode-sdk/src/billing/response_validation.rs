use serde_json::value::RawValue;
use std::collections::BTreeMap;

fn object(data: &RawValue) -> Option<BTreeMap<String, &RawValue>> {
    serde_json::from_str::<Option<_>>(data.get()).ok().flatten()
}

fn required(data: &RawValue, nullable: bool, keys: &[&str]) -> bool {
    object(data).is_some_and(|fields| {
        keys.iter().all(|key| {
            fields
                .get(*key)
                .is_some_and(|value| nullable || value.get() != "null")
        })
    })
}

fn rows(data: &RawValue, valid: impl Fn(&RawValue) -> bool) -> bool {
    serde_json::from_str::<Vec<&RawValue>>(data.get()).is_ok_and(|rows| rows.into_iter().all(valid))
}

pub(super) fn data_valid(operation: &str, data: Option<&RawValue>) -> bool {
    let Some(data) = data.filter(|data| data.get() != "null") else {
        return false;
    };
    match operation {
        "billing.ListBudgets" | "billing.ListBudgetThresholds" | "billing.ListBudgetAlerts" => {
            rows(data, |row| row.get() != "null")
        }
        "billing.GetBudget" | "billing.CreateBudget" | "billing.CreateBudgetThreshold" => {
            object(data)
                .and_then(|fields| fields.get("uuid").copied())
                .and_then(|uuid| serde_json::from_str::<String>(uuid.get()).ok())
                .is_some_and(|uuid| !uuid.trim().is_empty())
        }
        "billing.GetCurrentPeriodCost" => required(data, false, &["actualCost", "forecastedCost"]),
        "billing.GetBalances" => required(
            data,
            true,
            &["cash", "poc", "cashAvailable", "cashHolding", "pocHolding"],
        ),
        "billing.GetCostOverview" => object(data)
            .and_then(|fields| fields.get("summary").copied())
            .is_some_and(|summary| required(summary, false, &["currentCost"])),
        "billing.ListCostResources" => {
            object(data).is_some_and(|fields| {
                fields.get("pagination").is_some_and(|page| {
                    required(page, false, &["page", "size", "total", "totalPages"])
                }) && fields.get("summary").is_some_and(|summary| {
                    required(
                        summary,
                        false,
                        &["totalAll", "activeCount", "avgPerResource"],
                    )
                }) && fields
                    .get("items")
                    .is_some_and(|items| rows(items, |item| required(item, false, &["totalCost"])))
            })
        }
        _ => true,
    }
}
